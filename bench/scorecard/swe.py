"""SWE-bench Lite and Verified, retrieval only: the issue is the query, the gold is every file the reference
patch edits.

    uv run --with pyarrow python bench/scorecard/swe.py walk [--sets lite,verified] [--arms semlith,r0] [--repo R]
    uv run --with pyarrow python bench/scorecard/swe.py score <run dir>

`walk` checks out each instance's base commit (instances of one repository in commit order), re-indexes the
repository's store incrementally -- only the files that changed re-embed -- and runs every arm RUNS times.
Results append to <run dir>/rows.jsonl, one line per (instance, arm, run), so a walk that stops resumes where
it left off. `score` prints file Recall@1/5/10 (any gold file, and all gold files) and the share of instances
whose first gold file arrives within 2k/4k/8k tokens, median of the runs with the spread.
"""
import argparse, json, os, re, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import arms
from common import (BUDGETS, DATA, REPOS, RUNS, STORES, median_spread, parquet_rows, ranked_files, run_dir, sh,
                    tokens_at, write_manifest)

GOLD = re.compile(r"^diff --git a/(\S+) b/(\S+)$", re.M)


def instances(sets):
    out = {}
    for name in sets:
        for row in parquet_rows(os.path.join(DATA, "swe", f"{name}.parquet"),
                                ["instance_id", "repo", "base_commit", "patch", "problem_statement"]):
            iid = row["instance_id"]
            if iid not in out:
                out[iid] = dict(row, gold=sorted({b for _, b in GOLD.findall(row["patch"])}), sets=[])
            out[iid]["sets"].append(name)
    return out


def commit_time(repo_dir, sha):
    return int(sh(["git", "cat-file", "-p", sha], cwd=repo_dir).stdout.split("committer ")[1].split("\n")[0].split()[-2])


def checkout(repo_dir, sha):
    sh(["git", "checkout", "--quiet", "--force", sha], cwd=repo_dir)
    sh(["git", "clean", "-qfdx"], cwd=repo_dir)


def relativize(excerpts, root):
    prefix = root.rstrip("/") + "/"
    return [e._replace(path=e.path[len(prefix):] if e.path.startswith(prefix) else e.path) for e in excerpts]


ARMS = {
    "semlith": lambda q, root, store, files: relativize(arms.semlith(store, q), root),
    "r0": lambda q, root, store, files: arms.r0(q, root, files),
}


def walk(args):
    out = args.out or run_dir("swe")
    os.makedirs(out, exist_ok=True)
    rows_path = os.path.join(out, "rows.jsonl")
    done = set()
    if os.path.exists(rows_path):
        with open(rows_path) as f:
            for line in f:
                r = json.loads(line)
                done.add((r["instance_id"], r["arm"], r["run"]))
    insts = instances(args.sets.split(","))
    by_repo = {}
    for i in insts.values():
        by_repo.setdefault(i["repo"], []).append(i)
    arm_names = args.arms.split(",")
    for repo in sorted(by_repo):
        if args.repo and repo != args.repo:
            continue
        root = os.path.join(REPOS, repo.replace("/", "__"))
        store = os.path.join(STORES, "swe-" + repo.replace("/", "__"))
        todo = sorted(by_repo[repo], key=lambda i: commit_time(root, i["base_commit"]))
        for n, inst in enumerate(todo):
            need = [(a, r) for a in arm_names for r in range(RUNS) if (inst["instance_id"], a, r) not in done]
            if not need:
                continue
            t0 = time.time()
            checkout(root, inst["base_commit"])
            idx = sh([arms.SEMLITH, "index", "--store", store, "-q", root], timeout=7200).stdout.strip().splitlines()
            files = arms.store_files(store)
            present = [g for g in inst["gold"] if os.path.exists(os.path.join(root, g))]
            missing = [g for g in present if g not in set(files)]
            with open(rows_path, "a") as f:
                for a, r in need:
                    t = time.perf_counter()
                    ex = ARMS[a](inst["problem_statement"], root, store, files)
                    ms = (time.perf_counter() - t) * 1000
                    f.write(json.dumps({"instance_id": inst["instance_id"], "sets": inst["sets"], "repo": repo,
                                        "arm": a, "run": r, "ms": round(ms, 1), "gold": inst["gold"],
                                        "gold_not_indexed": missing, "files": ranked_files(ex)[:50],
                                        "first_gold_offset": next((e.offset for e in ex if e.path in inst["gold"]), None),
                                        "returned_bytes": ex[-1].offset if ex else 0, "index": idx[:1]}) + "\n")
            print(f"{repo} {n + 1}/{len(todo)} {inst['instance_id']} {time.time() - t0:.0f}s {idx[:1]}", flush=True)
    write_manifest(out, {"bench": "swe", "sets": args.sets, "arms": arm_names})
    return out


def score(directory, sets=("lite", "verified")):
    rows = [json.loads(l) for l in open(os.path.join(directory, "rows.jsonl"))]
    table = []
    for s in sets:
        for arm in sorted({r["arm"] for r in rows}):
            per_run = []
            for run in range(RUNS):
                rs = [r for r in rows if r["arm"] == arm and r["run"] == run and s in r["sets"]]
                if not rs:
                    continue
                m = {"n": len(rs)}
                for k in (1, 5, 10):
                    m[f"any@{k}"] = sum(bool(set(r["files"][:k]) & set(r["gold"])) for r in rs) / len(rs)
                    m[f"all@{k}"] = sum(set(r["gold"]) <= set(r["files"][:k]) for r in rs) / len(rs)
                for b in BUDGETS:
                    m[f"tok{b}"] = sum(r["first_gold_offset"] is not None and tokens_at(r["first_gold_offset"]) <= b
                                       for r in rs) / len(rs)
                hits = [tokens_at(r["first_gold_offset"]) for r in rs if r["first_gold_offset"] is not None]
                m["tokens_to_first_gold_median"] = sorted(hits)[len(hits) // 2] if hits else None
                per_run.append(m)
            if per_run:
                row = {"set": s, "arm": arm, "n": per_run[0]["n"], "runs": len(per_run)}
                for key in per_run[0]:
                    if key != "n" and per_run[0][key] is not None:
                        row[key], row[key + "_spread"] = median_spread([p[key] for p in per_run if p[key] is not None])
                table.append(row)
    with open(os.path.join(directory, "score.json"), "w") as f:
        json.dump(table, f, indent=1)
    for row in table:
        print(f"{row['set']:9} {row['arm']:10} n={row['n']:4} runs={row['runs']} "
              + " ".join(f"{k}={row[k]:.3f}" for k in ("any@1", "any@5", "any@10", "all@10", "tok2000", "tok4000", "tok8000"))
              + f" tokens_to_first_gold={row.get('tokens_to_first_gold_median')}")
    return table


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["walk", "score"])
    ap.add_argument("dir", nargs="?")
    ap.add_argument("--sets", default="lite,verified")
    ap.add_argument("--arms", default="semlith,r0")
    ap.add_argument("--repo")
    ap.add_argument("--out")
    a = ap.parse_args()
    if a.cmd == "walk":
        print(walk(a))
    else:
        score(a.dir)
