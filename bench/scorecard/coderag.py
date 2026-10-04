"""CodeRAG-Bench retrieval: NDCG@10 and Recall@10 on its canonical corpora, as its own
retrieval/create/*.py builds them.

    uv run --with pyarrow python bench/scorecard/coderag.py run [--tasks humaneval,mbpp,ds1000,odex,repoeval]
    uv run --with pyarrow python bench/scorecard/coderag.py score <run dir>

Every corpus document is written as one file and indexed as it is, so every arm ranks the benchmark's
own documents and the gold is the benchmark's own qrels:

- HumanEval: documents are prompt + canonical solution, queries the prompt.
- MBPP: documents are "# " + text + code, queries the text.
- DS-1000 (query: prompt) and ODEX (query: intent): the 34,003 library-documentation pages; gold the
  pages each problem names. Problems with no gold page have no qrels and are not scored.
- RepoEval, function level, 2k context: 20-line windows every 10 lines over the six repositories'
  Python files, identical windows merged; gold every window overlapping the task's context.

An arm's ranking is its documents in order of first appearance. NDCG uses binary relevance.
"""
import argparse, ast, glob, json, math, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import arms
from common import DATA, HOME, RUNS, STORES, median_spread, parquet_rows, run_dir, sh, write_manifest

CORPORA = os.path.join(HOME, "corpora")
REPOEVAL_REPOS = ["amazon-science_patchcore-inspection", "deepmind_tracr", "facebookresearch_omnivore",
                  "google_lightweight_mmm", "lucidrains_imagen-pytorch", "maxhumber_redframes"]


def coderag(name, columns=None):
    return parquet_rows(os.path.join(DATA, "coderag", f"{name}.parquet"), columns)


def windows(repos_dir, repos, size=20, step=10):
    """RepoEval's windows: [(repo, path tuple, start, end, text)], identical text merged to its first."""
    seen, out = {}, []
    for repo in repos:
        for fname in sorted(glob.glob(os.path.join(repos_dir, repo, "**", "*.py"), recursive=True)):
            try:
                lines = open(fname, encoding="utf8").read().splitlines()
            except (OSError, UnicodeDecodeError):
                continue
            rel = tuple(os.path.relpath(fname, repos_dir).split(os.sep))
            for line_no in range(0, len(lines), step):
                start, end = max(0, line_no - size // 2), min(len(lines), line_no + size - size // 2)
                text = "\n".join(lines[start:end])
                if not text:
                    continue
                if text in seen:
                    out[seen[text]][4].append((rel, start, end))
                    continue
                seen[text] = len(out)
                out.append([repo, rel, start, end, [(rel, start, end)], text])
    return out


def task(name):
    """(corpus name, [documents], extension, [(query id, query text, {gold document index})])."""
    if name == "humaneval":
        rows = coderag("humaneval")
        docs = [r["prompt"] + "\n" + r["canonical_solution"] for r in rows]
        return "humaneval", docs, "py", [(r["task_id"], r["prompt"], {i}) for i, r in enumerate(rows)]
    if name == "mbpp":
        rows = coderag("mbpp")
        docs = ["# " + r["text"] + "\n" + r["code"] for r in rows]
        return "mbpp", docs, "py", [(r["task_id"], r["text"], {i}) for i, r in enumerate(rows)]
    if name in ("ds1000", "odex"):
        lib = coderag("library-documentation")
        index = {r["doc_id"]: i for i, r in enumerate(lib)}
        docs = [r["doc_content"] for r in lib]
        field = "prompt" if name == "ds1000" else "intent"
        queries = []
        for i, r in enumerate(coderag(name)):
            gold = {index[d["title"]] for d in (r["docs"] or []) if d["title"] in index}
            if gold:
                queries.append((f"{name}-{i}", r[field], gold))
        return "library-documentation", docs, "txt", queries
    if name == "repoeval":
        repos_dir = os.path.join(DATA, "repoeval", "function_level")
        ws = windows(repos_dir, REPOEVAL_REPOS)
        docs = [w[5] for w in ws]
        tasks = [json.loads(l) for l in open(os.path.join(DATA, "repoeval", "datasets",
                                                          "function_level_completion_2k_context_codex.test.jsonl"))]
        queries = []
        for n, t in enumerate(tasks):
            m = t["metadata"]
            fpath = tuple(ast.literal_eval(m["fpath_tuple"]) if isinstance(m["fpath_tuple"], str) else m["fpath_tuple"])
            if fpath[0] not in REPOEVAL_REPOS:
                continue
            lo, hi = int(m["context_start_lineno"]), int(m["lineno"]) + 1
            gold = {i for i, w in enumerate(ws)
                    if any(p == fpath and not (s >= hi or e <= lo) for p, s, e in w[4])}
            if gold:
                queries.append((f"repoeval-{n}", t["prompt"], gold))
        return "repoeval", docs, "py", queries
    raise SystemExit(f"unknown task {name}")


def materialise(corpus, docs, ext):
    """Write each document as one file, once; the store indexes the directory."""
    root = os.path.join(CORPORA, corpus)
    done = os.path.join(root, ".written")
    if not os.path.exists(done):
        os.makedirs(root, exist_ok=True)
        for i, d in enumerate(docs):
            with open(os.path.join(root, f"{i:06d}.{ext}"), "w") as f:
                f.write(d)
        open(done, "w").write(str(len(docs)))
    return root


def doc_of(path):
    return int(os.path.basename(path).split(".")[0])


def ranking(excerpts):
    seen, out = set(), []
    for e in excerpts:
        d = doc_of(e.path)
        if d not in seen:
            seen.add(d)
            out.append(d)
    return out


def ndcg(ranked, gold, k=10):
    dcg = sum(1 / math.log2(i + 2) for i, d in enumerate(ranked[:k]) if d in gold)
    ideal = sum(1 / math.log2(i + 2) for i in range(min(len(gold), k)))
    return dcg / ideal


def run(args):
    out = args.out or run_dir("coderag")
    os.makedirs(out, exist_ok=True)
    rows_path = os.path.join(out, "rows.jsonl")
    done = set()
    if os.path.exists(rows_path):
        done = {(r["task"], r["qid"], r["arm"], r["run"]) for r in map(json.loads, open(rows_path))}
    for name in args.tasks.split(","):
        corpus, docs, ext, queries = task(name)
        root = materialise(corpus, docs, ext)
        store = os.path.join(STORES, "coderag-" + corpus)
        sh([arms.SEMLITH, "index", "--store", store, "-q", root], timeout=4 * 3600)
        files = sorted(f for f in os.listdir(root) if not f.startswith("."))
        mcp = arms.SemlithMCP(store)
        print(f"{name}: {len(queries)} queries over {len(docs)} documents", flush=True)
        with open(rows_path, "a") as f:
            for qid, text, gold in queries:
                for arm in args.arms.split(","):
                    for r in range(RUNS):
                        if (name, qid, arm, r) in done:
                            continue
                        if arm == "semlith":
                            ranked = ranking(mcp.search(text, k=100))
                        else:
                            ranked = [doc_of(p) for p in arms.bm25_files(text, root, files)]
                        f.write(json.dumps({"task": name, "qid": qid, "arm": arm, "run": r, "gold": sorted(gold),
                                            "ranked": ranked[:100]}) + "\n")
        mcp.close()
    write_manifest(out, {"bench": "coderag", "tasks": args.tasks, "arms": args.arms})
    return out


def score(directory):
    rows = [json.loads(l) for l in open(os.path.join(directory, "rows.jsonl"))]
    table = []
    for t in sorted({r["task"] for r in rows}):
        for arm in sorted({r["arm"] for r in rows}):
            per = []
            for run_no in range(RUNS):
                rs = [r for r in rows if r["task"] == t and r["arm"] == arm and r["run"] == run_no]
                if rs:
                    per.append({"ndcg@10": sum(ndcg(r["ranked"], set(r["gold"])) for r in rs) / len(rs),
                                "recall@10": sum(len(set(r["ranked"][:10]) & set(r["gold"])) / len(r["gold"])
                                                 for r in rs) / len(rs), "n": len(rs)})
            if per:
                row = {"task": t, "arm": arm, "n": per[0]["n"], "runs": len(per)}
                for key in ("ndcg@10", "recall@10"):
                    row[key], row[key + "_spread"] = median_spread([p[key] for p in per])
                table.append(row)
                print(f"{t:10} {arm:8} n={row['n']:4} runs={row['runs']} ndcg@10={row['ndcg@10']:.3f} "
                      f"recall@10={row['recall@10']:.3f} spread={row['ndcg@10_spread']:.3f}")
    json.dump(table, open(os.path.join(directory, "score.json"), "w"), indent=1)
    return table


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["run", "score"])
    ap.add_argument("dir", nargs="?")
    ap.add_argument("--tasks", default="humaneval,mbpp,ds1000,odex,repoeval")
    ap.add_argument("--arms", default="semlith,bm25")
    ap.add_argument("--out")
    a = ap.parse_args()
    print(run(a) if a.cmd == "run" else score(a.dir))
