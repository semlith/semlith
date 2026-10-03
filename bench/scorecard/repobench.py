"""RepoBench-R retrieval, test split: rank each instance's candidate snippets against its in-file code.

    uv run --with pyarrow python bench/scorecard/repobench.py run [--configs python_cff,...] [--keep 3,10]
    uv run --with pyarrow python bench/scorecard/repobench.py score <run dir>

As RepoBench's own run_repobench_r.py: the query is the last `keep` lines of the instance's `code`
(3 and 10, the two its archive runs), the candidates are its `context` snippets, and acc@k is whether
`gold_snippet_index` is among the top k. Easy instances have fewer candidates, hard ones ten or more.

semlith ranks a candidate by its first appearance in `semlith_search` scoped to the instance's own
candidates (each written as one file); candidates the search does not return keep their original
order after it, which is what a tie means. bm25 is BM25 over the same candidates, same tokens as
the other benchmarks' grep baseline.
"""
import argparse, glob, json, math, os, shutil, sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import arms
from common import DATA, HOME, RUNS, STORES, median_spread, parquet_rows, run_dir, sh, write_manifest

CONFIGS = [f"{lang}_{kind}" for lang in ("python", "java") for kind in ("cff", "cfr")]
CORPORA = os.path.join(HOME, "corpora", "repobench")


SEED = 20261003


def instances(config, sample=0):
    """(level, row) for the config's test split; `sample` > 0 takes that many per level, by SEED.

    Indexing the candidates costs about 1.2 s an instance on the M1, so the whole 48,000-instance
    test split is about 16 hours; the scorecard states the sample size beside every row.
    """
    import random
    out = []
    for level in ("easy", "hard"):
        rows = []
        for f in sorted(glob.glob(os.path.join(DATA, "repobench", config, f"test_{level}-*.parquet"))):
            rows.extend(parquet_rows(f, ["code", "context", "gold_snippet_index"]))
        total = len(rows)
        if sample and sample < total:
            rows = [rows[i] for i in sorted(random.Random(f"{SEED}-{config}-{level}").sample(range(total), sample))]
        out.extend((level, row) for row in rows)
    return out


def crop(code, keep):
    return "\n".join(code.splitlines()[-keep:])


def bm25_rank(query, candidates):
    terms = [t.lower() for t, _ in arms.terms(query)]
    weight = {t.lower(): w for t, w in arms.terms(query)}
    docs = [c.lower() for c in candidates]
    n, avg = len(docs), max(sum(map(len, docs)) / max(len(docs), 1), 1.0)
    df = Counter(t for t in terms for d in docs if t in d)

    def score(d):
        norm = 1.2 * (0.25 + 0.75 * len(d) / avg)
        total = 0.0
        for t in terms:
            c = d.count(t)
            if c:
                total += weight[t] * math.log(1 + (n - df[t] + 0.5) / (df[t] + 0.5)) * c * 2.2 / (c + norm)
        return total

    return sorted(range(n), key=lambda i: (-score(docs[i]), i))


def run(args):
    out = args.out or run_dir("repobench")
    os.makedirs(out, exist_ok=True)
    rows_path = os.path.join(out, "rows.jsonl")
    done = set()
    if os.path.exists(rows_path):
        done = {(r["config"], r["i"], r["keep"], r["arm"], r["run"]) for r in map(json.loads, open(rows_path))}
    keeps = [int(k) for k in args.keep.split(",")]
    for config in args.configs.split(","):
        insts = instances(config, args.sample)
        if args.limit:
            # A smoke run: the first and last `limit` instances, so both levels are in it.
            insts = insts[:args.limit] + insts[-args.limit:]
        ext = "py" if config.startswith("python") else "java"
        root = os.path.join(CORPORA, config + (f"-limit{args.limit}" if args.limit else "")
                            + (f"-sample{args.sample}" if args.sample else ""))
        if not os.path.exists(os.path.join(root, ".written")):
            for i, (_, row) in enumerate(insts):
                d = os.path.join(root, f"i{i:05d}")
                os.makedirs(d, exist_ok=True)
                for j, cand in enumerate(row["context"]):
                    with open(os.path.join(d, f"c{j:03d}.{ext}"), "w") as f:
                        f.write(cand)
            open(os.path.join(root, ".written"), "w").write(str(len(insts)))
        store = os.path.join(STORES, "repobench-" + os.path.basename(root))
        sh([arms.SEMLITH, "index", "--store", store, "-q", root], timeout=8 * 3600)
        mcp = arms.SemlithMCP(store)
        print(f"{config}: {len(insts)} instances", flush=True)
        with open(rows_path, "a") as f:
            for i, (level, row) in enumerate(insts):
                cands = row["context"]
                for keep in keeps:
                    query = crop(row["code"], keep)
                    for arm in args.arms.split(","):
                        for r in range(RUNS):
                            if (config, i, keep, arm, r) in done:
                                continue
                            if arm == "semlith":
                                hits = mcp.search(query, k=len(cands) + 10, paths=[f"i{i:05d}/**"])
                                order = []
                                for e in hits:
                                    j = int(os.path.basename(e.path)[1:4])
                                    if j not in order:
                                        order.append(j)
                                ranked = order + [j for j in range(len(cands)) if j not in order]
                            else:
                                ranked = bm25_rank(query, cands)
                            f.write(json.dumps({"config": config, "level": level, "i": i, "keep": keep, "arm": arm,
                                                "run": r, "gold": int(row["gold_snippet_index"]),
                                                "candidates": len(cands), "ranked": ranked[:10]}) + "\n")
        mcp.close()
        if args.forget:
            shutil.rmtree(store, ignore_errors=True)
    write_manifest(out, {"bench": "repobench", "configs": args.configs, "keep": args.keep, "arms": args.arms,
                         "sample_per_level": args.sample, "seed": SEED})
    return out


def score(directory):
    rows = [json.loads(l) for l in open(os.path.join(directory, "rows.jsonl"))]
    table = []
    keys = sorted({(r["config"], r["level"], r["keep"], r["arm"]) for r in rows})
    for config, level, keep, arm in keys:
        per = []
        for run_no in range(RUNS):
            rs = [r for r in rows if (r["config"], r["level"], r["keep"], r["arm"], r["run"]) ==
                  (config, level, keep, arm, run_no)]
            if rs:
                per.append({f"acc@{k}": sum(r["gold"] in r["ranked"][:k] for r in rs) / len(rs) for k in (1, 3, 5)}
                           | {"n": len(rs)})
        row = {"config": config, "level": level, "keep": keep, "arm": arm, "n": per[0]["n"], "runs": len(per)}
        for k in ("acc@1", "acc@3", "acc@5"):
            row[k], row[k + "_spread"] = median_spread([p[k] for p in per])
        table.append(row)
        print(f"{config:11} {level:4} keep={keep:2} {arm:8} n={row['n']:5} runs={row['runs']} "
              f"acc@1={row['acc@1']:.3f} acc@3={row['acc@3']:.3f} acc@5={row['acc@5']:.3f} spread={row['acc@1_spread']:.3f}")
    json.dump(table, open(os.path.join(directory, "score.json"), "w"), indent=1)
    return table


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["run", "score"])
    ap.add_argument("dir", nargs="?")
    ap.add_argument("--configs", default=",".join(CONFIGS))
    ap.add_argument("--keep", default="3,10")
    ap.add_argument("--arms", default="semlith,bm25")
    ap.add_argument("--forget", action="store_true", help="delete each config's store after scoring it")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--sample", type=int, default=500, help="instances per config and level; 0 for all")
    ap.add_argument("--out")
    a = ap.parse_args()
    print(run(a) if a.cmd == "run" else score(a.dir))
