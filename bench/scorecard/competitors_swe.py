"""Competitor arms (competitors/ALL) on the first SAMPLE_N instances of the seeded 50-instance SWE-bench Lite
sample, retrieval only, beside semlith on the same checkouts.

    cd bench/scorecard && python3.11 competitors_swe.py walk [--tools rg,ugrep,...] [--out DIR]
    cd bench/scorecard && python3.11 competitors_swe.py score [--out DIR]

`walk` keeps its own clone of each repository under OUT/repos, checks out each sample instance's base commit
(per repository, in commit order), indexes semlith's store (OUT/stores/semlith-<repo>, timed) and then each tool
(work = OUT/work/<tool>/<repo>, so incremental tools stay incremental), and searches the problem statement RUNS
times per tool. One row per (instance, arm, run) appends to OUT/rows.jsonl; a stopped walk resumes. A failure or
timeout is an `error` on the row. A tool whose cumulative index seconds pass INDEX_BUDGET x semlith's on the
same instances is stopped, and its remaining instances get rows marked "not run: index budget".

`score` reports, per tool on the instances it completed (every run without error), file Recall@1/5/10 (any
gold), all-gold@10, the share whose first gold file arrives within 2k/4k/8k tokens and the median tokens to the
first gold, median of runs with spread -- and semlith on exactly that subset beside it. Results render as
arms.collect renders them, so the token budgets compare across arms.
"""
import argparse, json, os, subprocess, sys, time
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import arms, swe
from common import BUDGETS, HOME, REPOS, RUNS, median_spread, ranked_files, sh, tokens_at, write_manifest
from competitors import ALL, _common

OUT = os.path.join(HOME, "competitors")
SAMPLE = os.path.join(HOME, "agent", "sample.json")
# The sample is a seeded shuffle, so its prefix is still a random sample. 20 since US-SEMLITH-0.38.0-I01: the slow four
# would not finish 50 in the release's wall clock.
SAMPLE_N = 20
INDEX_BUDGET = 4  # x semlith's cumulative index seconds on the same instances
FAST_FIRST = ["rg", "ugrep", "semble", "graphify", "serena", "sourcebot", "grepai", "colgrep", "ck"]


def sample():
    ids = [i["instance_id"] for i in json.load(open(SAMPLE))][:SAMPLE_N]
    lite = swe.instances(["lite"])
    return [lite[i] for i in ids]


def load_rows(out):
    p = os.path.join(out, "rows.jsonl")
    if not os.path.exists(p):
        return []
    last = {}  # one row per (instance, arm, run); a later line wins
    for l in open(p):
        r = json.loads(l)
        last[(r["instance_id"], r["arm"], r["run"])] = r
    return list(last.values())


def clone(out, repo):
    name = repo.replace("/", "__")
    dst = os.path.join(out, "repos", name)
    if not os.path.isdir(dst):
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        sh(["git", "clone", "--quiet", os.path.join(REPOS, name), dst])
    return dst


def ensure_commit(root, sha):
    if sh(["git", "cat-file", "-e", sha + "^{commit}"], cwd=root, check=False).returncode:
        sh(["git", "fetch", "-q", "origin", sha], cwd=root)


def normalise(items, root, work):
    """Root-relative paths; counts the absolute ones a tool returned (an adapter defect worth reporting)."""
    out, absolute = [], 0
    bases = [root.rstrip("/") + "/", os.path.join(work, "mirror") + "/"]
    for path, start, end, text in items:
        if os.path.isabs(path):
            absolute += 1
            for b in bases:
                if path.startswith(b):
                    path = path[len(b):]
                    break
        out.append((path, start, end, text or ""))
    return out, absolute


def row(inst, arm, run, **kw):
    r = {"instance_id": inst["instance_id"], "repo": inst["repo"], "arm": arm, "run": run, "gold": inst["gold"],
         "files": [], "first_gold_offset": None, "returned_bytes": 0, "results": 0, "ms": None, "index_s": None,
         "error": None}
    r.update(kw)
    return r


def scored(inst, items, ms, index_s, absolute=0):
    ex = arms.collect(items)
    last = ex[-1] if ex else None
    return dict(files=ranked_files(ex)[:50], ms=round(ms, 1), index_s=index_s, results=len(ex),
                first_gold_offset=next((e.offset for e in ex if e.path in inst["gold"]), None),
                returned_bytes=last.offset + len(arms.render(*last[:4]).encode()) if last else 0,
                abs_paths=absolute, empty=not ex)


def serena_project(root):
    """`serena project index` auto-creates a project and, when a repo has C/JS files too, asks on stdin which
    extra language servers to enable -- with no tty it dies on EOF. Create it Python-only first, as on testrepo."""
    from competitors import serena, _common
    if not os.path.exists(os.path.join(_common.TOOLS, "work", "serena", os.path.basename(root), ".serena", "project.yml")):
        _common.run([serena.BIN, "project", "create", root, "--language", "python"], cwd=root, env_=serena._env(),
                    timeout=_common.INDEX_TIMEOUT)


def budget(rows, tool, ref=None):
    """(tool's cumulative index seconds, semlith's on the same instances). `ref` is another walk's semlith index
    seconds by instance, preferred to this walk's own: semlith's vector cache makes a second walk's first index of
    a repository a few seconds where the first walk paid the cold index, and the budget is against the cold one."""
    sem = {r["instance_id"]: r["index_s"] or 0 for r in rows if r["arm"] == "semlith" and r["run"] == 0}
    sem.update(ref or {})
    mine = {r["instance_id"]: r["index_s"] or 0 for r in rows
            if r["arm"] == tool and r["run"] == 0 and r["index_s"] is not None}
    return sum(mine.values()), sum(sem.get(i, 0) for i in mine)


def walk(args):
    out = args.out
    os.makedirs(out, exist_ok=True)
    rows = load_rows(out)
    done = {(r["instance_id"], r["arm"], r["run"]) for r in rows}
    tools = [t for t in args.tools.split(",") if t]
    for t in tools:
        why = ALL[t].available()
        if why:
            print(f"{t}: unavailable, skipped: {why}", flush=True)
    tools = [t for t in tools if not ALL[t].available()]
    def reference():  # re-read per instance: the other walk is usually still running
        return {r["instance_id"]: r["index_s"] or 0 for r in load_rows(args.budget_from)
                if r["arm"] == "semlith" and r["run"] == 0 and not r["error"]} if args.budget_from else {}
    stopped = set()
    insts = sample()
    f = open(os.path.join(out, "rows.jsonl"), "a")
    import fcntl
    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)  # one walk at a time: two would share the tools' work dirs

    def emit(r):
        rows.append(r)
        done.add((r["instance_id"], r["arm"], r["run"]))
        f.write(json.dumps(r) + "\n")
        f.flush()

    def stop(tool, cum, sem):
        stopped.add(tool)
        print(f"{tool}: index budget spent ({cum:.0f}s > {INDEX_BUDGET} x semlith {sem:.0f}s), stopping", flush=True)
        for i in insts:
            for run in range(RUNS):
                if (i["instance_id"], tool, run) not in done:
                    emit(row(i, tool, run, error="not run: index budget"))

    for t in tools:  # a resumed walk keeps a stop it already made
        if any(r["arm"] == t and r["error"] == "not run: index budget" for r in rows):
            stopped.add(t)

    by_repo = {}
    for i in insts:
        by_repo.setdefault(i["repo"], []).append(i)
    t_walk = time.time()
    for repo in sorted(by_repo):
        if args.repo and repo != args.repo:
            continue
        root = clone(out, repo)
        for i in by_repo[repo]:
            ensure_commit(root, i["base_commit"])
        todo = sorted(by_repo[repo], key=lambda i: swe.commit_time(root, i["base_commit"]))
        store = os.path.join(out, "stores", "semlith-" + repo.replace("/", "__"))
        for n, inst in enumerate(todo):
            iid = inst["instance_id"]
            need_sem = [r for r in range(RUNS) if (iid, "semlith", r) not in done]
            need = {t: [r for r in range(RUNS) if (iid, t, r) not in done] for t in tools if t not in stopped}
            need = {t: rs for t, rs in need.items() if rs}
            if not need_sem and not need:
                continue
            t0 = time.time()
            ref = reference()
            swe.checkout(root, inst["base_commit"])
            if need_sem:
                t = time.time()
                try:
                    swe.index(store, root)
                    arms.embedded(store)
                    idx, err = time.time() - t, None
                except Exception as e:
                    idx, err = time.time() - t, f"index: {type(e).__name__}: {str(e)[-500:]}"
                for run in need_sem:
                    if err:
                        emit(row(inst, "semlith", run, index_s=round(idx, 1), error=err))
                        continue
                    t = time.perf_counter()
                    try:
                        ex = swe.relativize(arms.semlith(store, inst["problem_statement"], k=100), root)
                        items = [(e.path, e.start, e.end, e.text) for e in ex]
                        emit(row(inst, "semlith", run, **scored(inst, items, (time.perf_counter() - t) * 1000,
                                                                round(idx, 1))))
                    except Exception as e:
                        emit(row(inst, "semlith", run, index_s=round(idx, 1),
                                 error=f"search: {type(e).__name__}: {str(e)[-500:]}"))
            for tool, runs in need.items():
                mod = ALL[tool]
                work = os.path.join(out, "work", tool, repo.replace("/", "__"))
                os.makedirs(work, exist_ok=True)
                # The budget is a live limit, not only a check after the index: an index that would pass
                # 4x semlith's cumulative seconds is stopped where it crosses them, not at the 2 h cap.
                cum, sem = budget(rows, tool, ref)
                sem += ref.get(iid, next((r["index_s"] or 0 for r in rows if r["instance_id"] == iid
                                          and r["arm"] == "semlith" and r["run"] == 0), 0))
                _common.INDEX_TIMEOUT = min(2 * 3600, max(INDEX_BUDGET * sem - cum, 120))
                t = time.time()
                try:
                    if tool == "serena":
                        serena_project(root)
                    mod.index(root, work)
                    idx, err = time.time() - t, None
                except Exception as e:
                    idx, err = time.time() - t, f"index: {type(e).__name__}: {str(e)[-500:]}"
                for run in runs:
                    if err:
                        emit(row(inst, tool, run, index_s=round(idx, 1), error=err))
                        continue
                    t = time.perf_counter()
                    try:
                        items, absolute = normalise(mod.search(inst["problem_statement"], root, work, k=50), root, work)
                        emit(row(inst, tool, run, **scored(inst, items, (time.perf_counter() - t) * 1000,
                                                           round(idx, 1), absolute)))
                    except Exception as e:
                        emit(row(inst, tool, run, index_s=round(idx, 1),
                                 error=f"search: {type(e).__name__}: {str(e)[-500:]}"))
                        # A search past its 120 s timeout is not retried RUNS times: the later runs would only
                        # spend another 2+ minutes each to time out again.
                        if isinstance(e, (TimeoutError, subprocess.TimeoutExpired)):
                            err = "search: not run, an earlier run timed out"
                cum, sem = budget(rows, tool, ref)
                if cum > INDEX_BUDGET * sem and cum > 60:  # ponytail: 60 s floor so a 0.1 s jitter on an index-free tool never trips it
                    stop(tool, cum, sem)
                print(f"  {tool:9} idx {idx:7.1f}s cum {cum:7.0f}s/semlith {sem:6.0f}s {err or ''}"[:300], flush=True)
            print(f"{repo} {n + 1}/{len(todo)} {iid} {time.time() - t0:.0f}s (walk {time.time() - t_walk:.0f}s)",
                  flush=True)
    f.close()
    write_manifest(out, {"bench": "swe-competitors", "sample": SAMPLE, "sample_n": SAMPLE_N, "tools": tools, "runs": RUNS,
                         "index_budget_x": INDEX_BUDGET,
                         "versions": {t: ALL[t].version() for t in tools}})


def metrics(rs):
    m = {"n": len(rs)}
    for k in (1, 5, 10):
        m[f"any@{k}"] = sum(bool(set(r["files"][:k]) & set(r["gold"])) for r in rs) / len(rs)
    m["all@10"] = sum(set(r["gold"]) <= set(r["files"][:10]) for r in rs) / len(rs)
    for b in BUDGETS:
        m[f"tok{b}"] = sum(r["first_gold_offset"] is not None and tokens_at(r["first_gold_offset"]) <= b
                           for r in rs) / len(rs)
    hits = sorted(tokens_at(r["first_gold_offset"]) for r in rs if r["first_gold_offset"] is not None)
    m["tok_median"] = hits[len(hits) // 2] if hits else None
    return m


def summarise(rows, arm, ids):
    per_run = []
    for run in range(RUNS):
        rs = [r for r in rows if r["arm"] == arm and r["run"] == run and r["instance_id"] in ids and not r["error"]]
        if rs:
            per_run.append(metrics(rs))
    if not per_run:
        return None
    out = {"n": per_run[0]["n"], "runs": len(per_run)}
    for key in per_run[0]:
        vals = [p[key] for p in per_run if p[key] is not None]
        if key != "n" and vals:
            out[key], out[key + "_spread"] = median_spread(vals)
    return out


COLS = ("any@1", "any@5", "any@10", "all@10", "tok2000", "tok4000", "tok8000")


def score(args):
    """One table over --out and every --also directory: a second walk run beside the first (the slow tools in
    their own directory, so the two walks never share rows.jsonl) scores its tools against its own semlith rows."""
    table, allsem = tabulate(load_rows(args.out))
    for d in args.also or []:
        more, _ = tabulate(load_rows(d))
        table += [t for t in more if t["tool"] not in {x["tool"] for x in table}]
    table.sort(key=lambda t: FAST_FIRST.index(t["tool"]))
    insts = [i["instance_id"] for i in sample()]
    with open(os.path.join(args.out, "score.json"), "w") as f:
        json.dump({"tools": table, "semlith_all": allsem, "runs": RUNS, "budgets": BUDGETS}, f, indent=1)

    def fmt(m):
        if not m:
            return " ".join(f"{'-':>6}" for _ in COLS) + f" {'-':>6}"
        return " ".join(f"{m.get(c, 0):6.2f}" for c in COLS) + f" {str(m.get('tok_median')):>6}"
    head = " ".join(f"{c:>6}" for c in COLS) + f" {'tokmed':>6}"
    print(f"{'tool':10} {'n':>5} {'arm':8} {head}  idx_s  not run / partial")
    for t in table:
        nr = "; ".join(f"{v} {k}" for k, v in t["not_run"].items()) or "-"
        print(f"{t['tool']:10} {t['completed']:2}/{t['of']:2} {'tool':8} {fmt(t['tool_metrics'])} {t['index_s_total']:7.0f}  {nr}")
        print(f"{'':10} {'':5} {'semlith':8} {fmt(t['semlith_metrics'])} {t['semlith_index_s_total']:7.0f}")
    if allsem:
        print(f"{'semlith':10} {allsem['n']:2}/{len(insts):2} {'all':8} {fmt(allsem)}")
    return table


def tabulate(rows):
    insts = [i["instance_id"] for i in sample()]
    table = []
    for tool in [t for t in FAST_FIRST if any(r["arm"] == t for r in rows)]:
        ok, why = set(), Counter()
        for iid in insts:
            rs = [r for r in rows if r["arm"] == tool and r["instance_id"] == iid]
            errs = [r["error"] for r in rs if r["error"]]
            sem_ok = sum(1 for r in rows if r["arm"] == "semlith" and r["instance_id"] == iid and not r["error"])
            if len(rs) >= RUNS and not errs and sem_ok >= RUNS:
                ok.add(iid)
            else:
                why[(errs[0][:70] if errs else "no rows" if len(rs) < RUNS else "semlith error")] += 1
        abs_paths = sum(r.get("abs_paths") or 0 for r in rows if r["arm"] == tool)
        empty = sum(1 for r in rows if r["arm"] == tool and r.get("empty") and r["instance_id"] in ok)
        idx = sum(r["index_s"] or 0 for r in rows if r["arm"] == tool and r["run"] == 0 and r["instance_id"] in ok)
        sidx = sum(r["index_s"] or 0 for r in rows if r["arm"] == "semlith" and r["run"] == 0 and r["instance_id"] in ok)
        ms = sorted(r["ms"] for r in rows if r["arm"] == tool and r["instance_id"] in ok and r["ms"] is not None)
        sms = sorted(r["ms"] for r in rows if r["arm"] == "semlith" and r["instance_id"] in ok and r["ms"] is not None)
        table.append({"tool": tool, "completed": len(ok), "of": len(insts), "not_run": dict(why),
                      "abs_paths_returned": abs_paths, "empty_result_rows": empty,
                      "index_s_total": round(idx, 1), "semlith_index_s_total": round(sidx, 1),
                      "search_ms_median": ms[len(ms) // 2] if ms else None,
                      "semlith_search_ms_median": sms[len(sms) // 2] if sms else None,
                      "tool_metrics": summarise(rows, tool, ok) if ok else None,
                      "semlith_metrics": summarise(rows, "semlith", ok) if ok else None})
    return table, summarise(rows, "semlith", set(insts))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["walk", "score"])
    ap.add_argument("--tools", default=",".join(FAST_FIRST))
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--repo", help="one repository, e.g. psf/requests")
    ap.add_argument("--budget-from", help="walk: take semlith's index seconds from this walk's directory")
    ap.add_argument("--also", action="append", help="score: another walk's directory to merge, e.g. a slow-tools walk")
    a = ap.parse_args()
    walk(a) if a.cmd == "walk" else score(a)
