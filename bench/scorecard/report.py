"""The scorecard tables, from the runs' own score files.

    python bench/scorecard/report.py [--swe DIR] [--coderag DIR] [--repobench DIR] [--competitors FILE]
                                     [--agent FILE] [--documents DIR] [--ledger DIR] [--readme bench/scorecard/README.md]

Each option defaults to the newest run under SCORECARD_HOME/results (the competitor and agent files to
their fixed paths; the documents round to its newest score committed under bench/scorecard/documents/runs). Prints Markdown -- or, with --readme, writes it between that file's scorecard markers:
one table per benchmark, each with its instance count, runs and the command that reproduces it. The tables
live on the scorecard page, `bench/scorecard/README.md`; `charts.py` then draws the charts from them. A number is
the median of the runs; a spread other than zero is printed beside it.
"""
import argparse, glob, json, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import HOME, RESULTS

PCT = lambda x: f"{100 * x:.1f} %"


def newest(bench):
    runs = sorted(glob.glob(os.path.join(RESULTS, bench, "*", "score.json")), key=os.path.getmtime)
    return os.path.dirname(runs[-1]) if runs else None


def cell(row, key, fmt=PCT):
    v = row.get(key)
    if v is None:
        return "—"
    s = row.get(key + "_spread") or 0
    return fmt(v) + (f" ±{fmt(s)}" if s else "")


def command(directory):
    try:
        return json.load(open(os.path.join(directory, "MANIFEST.json")))["command"].replace(os.path.expanduser("~"), "~")
    except (OSError, KeyError, ValueError):
        return "see bench/scorecard/README.md"


def table(head, rows):
    out = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out)


ARM = {"semlith": "semlith", "r0": "grep then read", "bm25": "BM25 (grep baseline)"}


def swe(directory):
    rows = json.load(open(os.path.join(directory, "score.json")))
    body = []
    for r in rows:
        body.append([{"lite": "Lite", "verified": "Verified"}[r["set"]], ARM.get(r["arm"], r["arm"]), r["n"],
                     cell(r, "any@1"), cell(r, "any@5"), cell(r, "any@10"), cell(r, "all@10"),
                     cell(r, "tok4000"), r.get("tokens_to_first_gold_median", "—")])
    return ("### SWE-bench, retrieval only\n\nThe issue is the query; the gold is every file the reference patch "
            "edits; each instance is indexed at its own base commit.\n\n"
            + table(["set", "arm", "instances", "file hit@1", "@5", "@10", "all gold @10", "first gold within 4k tokens",
                     "median tokens to first gold"], body)
            + "\n\n`uv run --with pyarrow python bench/scorecard/swe.py walk && uv run --with pyarrow python "
            "bench/scorecard/swe.py score <run dir>...`")


def coderag(directory):
    rows = json.load(open(os.path.join(directory, "score.json")))
    by = {(r["task"], r["arm"]): r for r in rows}
    f3 = lambda x: f"{x:.3f}"
    body = []
    for task in sorted({r["task"] for r in rows}):
        b, m = by.get((task, "bm25"), {}), by.get((task, "semlith"), {})
        body.append([task, (m or b).get("n", "—"), cell(b, "ndcg@10", f3), cell(m, "ndcg@10", f3),
                     cell(b, "recall@10", f3), cell(m, "recall@10", f3)])
    return ("### CodeRAG-Bench, retrieval\n\nEach task's canonical corpus and gold documents.\n\n"
            + table(["task", "queries", "NDCG@10 BM25", "NDCG@10 semlith", "Recall@10 BM25", "Recall@10 semlith"],
                    body) + f"\n\n`{command(directory)}`")


def repobench(directory):
    rows = json.load(open(os.path.join(directory, "score.json")))
    manifest = {}
    try:
        manifest = json.load(open(os.path.join(directory, "MANIFEST.json")))
    except (OSError, ValueError):
        pass
    by = {(r["config"], r["level"], r["keep"], r["arm"]): r for r in rows}
    body = []
    for config, level, keep in sorted({(r["config"], r["level"], r["keep"]) for r in rows}):
        b, m = by.get((config, level, keep, "bm25"), {}), by.get((config, level, keep, "semlith"), {})
        body.append([config.replace("_cff", " first").replace("_cfr", " random"), level, keep, (m or b).get("n", "—"),
                     cell(b, "acc@1"), cell(m, "acc@1"), cell(b, "acc@5"), cell(m, "acc@5")])
    sample = manifest.get("sample_per_level")
    note = (f"A seeded sample of {sample} instances per configuration and level (seed {manifest.get('seed')}) of "
            "the 48 000-instance test split." if sample else "The whole test split.")
    return ("### RepoBench-R\n\nThe query is the last `keep` lines of the in-file code; the candidates are the "
            f"instance's own cross-file snippets. {note}\n\n"
            + table(["setting", "level", "keep", "instances", "acc@1 BM25", "acc@1 semlith", "acc@5 BM25",
                     "acc@5 semlith"], body)
            + f"\n\n`{command(directory)}`")


def competitors(path):
    doc = json.load(open(path))
    versions = {}
    for manifest in sorted(glob.glob(os.path.join(HOME, "competitors*", "MANIFEST.json"))):  # each walk records its own tools
        try:
            versions.update({t: v for t, v in json.load(open(manifest))["versions"].items() if v})
        except (OSError, KeyError, ValueError):
            pass
    m = lambda r, key: cell(r, key) if r else "—"
    body = []
    for t in doc["tools"]:
        tm, sm = t["tool_metrics"], t["semlith_metrics"]
        nr = "; ".join(f"{v}: {': '.join(k.split(': ')[:2])}" for k, v in t["not_run"].items())  # kind, not the command
        body.append([t["tool"], versions.get(t["tool"], ""), f"{t['completed']}/{t['of']}", m(tm, "any@1"), m(tm, "any@5"),
                     m(tm, "any@10"), m(tm, "tok4000"), m(sm, "any@5"), m(sm, "tok4000"), nr or "—"])
    a = doc.get("semlith_all")
    if a:
        body.append(["semlith, every instance", "", f"{a['n']}", cell(a, "any@1"), cell(a, "any@5"), cell(a, "any@10"),
                     cell(a, "tok4000"), "", "", ""])
    return ("### Competitors, SWE-bench Lite\n\nThe same base-commit checkouts and issue text for every tool, on a "
            "seeded random sample of SWE-bench Lite; semlith beside each tool on exactly the instances that tool "
            "completed (all runs without an error). A tool whose cumulative indexing passed 4x semlith's is stopped "
            "and its remaining instances are \"not run\".\n\n"
            + table(["tool", "version", "completed", "file hit@1", "@5", "@10", "first gold within 4k tokens",
                     "semlith hit@5, same instances", "semlith within 4k, same instances", "not run"], body)
            + "\n\n`cd bench/scorecard && python3.11 competitors_swe.py walk && python3.11 competitors_swe.py score` (the slow "
            "tools in their own `--out` with `SEMLITH_VECTOR_CACHE_MB=0`, scored with `--also`; see bench/scorecard/README.md)")


ARMS_AGENT = {"g": "grep (Grep, Glob, Read)", "s": "semlith installed, Grep built in",
              "gate": "semlith installed, Grep gated to semlith first"}


def agent(path):
    order = list(ARMS_AGENT)
    rows = sorted(json.load(open(path)), key=lambda r: (r["model"] != "opus", order.index(r["arm"])))
    money = lambda x: f"${x:.3f}"
    count = lambda x: f"{x:g}"
    body = [[r["model"], ARMS_AGENT.get(r["arm"], r["arm"]), r["runs"], max(r["n"]), cell(r, "hit"),
             cell(r, "cost_per_session", money), cell(r, "cost_per_correct", money),
             cell(r, "tokens_in_median", count), cell(r, "used_semlith", count)] for r in rows]
    return ("### Agents on an unfamiliar codebase\n\nHeadless Claude Code on 50 held-out questions generated from the "
            "code of a 70-repository corpus (not a public set: Opus has memorised SWE-bench Lite, "
            "`bench/scorecard/agent/contamination.md`).\n\n"
            + table(["model", "arm", "runs", "questions", "correct", "cost / session", "cost / correct answer",
                     "median input tokens", "sessions calling semlith"], body)
            + "\n\n`python3 bench/scorecard/agent/corpus_round.py`")


DOCUMENT_RUNS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "documents", "runs")
ARMS_DOCUMENTS = {"r0": "rg, BM25", "r0x": "rga (rg with document adapters), BM25", "s1": "semlith_search, k 50",
                  "s2": "semlith_brief", "g": "grep (Grep, Glob, Read)", "s": "semlith installed, Grep built in",
                  "sx": "semlith and Read only"}


def newest_documents(name):
    """The newest committed run holding `name`; run directories are named by their start time."""
    runs = sorted(glob.glob(os.path.join(DOCUMENT_RUNS, "*", name)))
    return os.path.dirname(runs[-1]) if runs else None


def documents(directory):
    """The deterministic arms from `directory`; the agent round from the newest committed one, when there is one."""
    rows = json.load(open(os.path.join(directory, "score.json")))
    agent_dir = newest_documents("agent.json")
    of = lambda r, key: f"{r[key]}/{r['n']}" + (f" ±{r[key + '_spread']}" if r.get(key + "_spread") else "")
    body = [[ARMS_DOCUMENTS.get(r["arm"], r["arm"]), r["runs"], of(r, "hit2000"), of(r, "hit4000"), of(r, "hit8000"),
             of(r, "file4000"), r["first_tokens_median"] if r["first_tokens_median"] is not None else "—"] for r in rows]
    out = ("### Documents\n\n127 questions about passages inside 53 public documents fetched from their publishers' "
           "own URLs, in 11 formats: PDF, Word, PowerPoint, Excel, OpenDocument text, sheets and slides, EPUB, RTF, "
           "notebooks and CSV (two mailing-list archives sit beside them, with no questions). A passage is found when a delivered excerpt is in the gold document and shares a run of six "
           "words with the gold passage, whichever extractor produced it.\n\n"
           + table(["arm", "runs", "found within 2k tokens", "4k", "8k", "right document within 4k",
                    "median tokens to the first find"], body)
           + "\n\n`python3 bench/scorecard/documents/documents.py run && python3 bench/scorecard/documents/documents.py "
           "score <run dir>`")
    if not agent_dir:
        return out + "\n\nThe agent round on these questions has not been run on this release."
    agents = json.load(open(os.path.join(agent_dir, "agent.json")))
    money = lambda x: f"${x:.3f}"
    body = [[r["model"], ARMS_DOCUMENTS.get(r["arm"], r["arm"]), r["runs"], max(r["n"]), of(r | {"n": max(r["n"])}, "hit"),
             cell(r, "cost_per_session", money), r["used_semlith"] if r["arm"] != "g" else "—",
             f"{r['quotes_not_in_document']} of {r['quotes_checked']}"] for r in agents]
    return (out + "\n\nHeadless Claude Code on the same questions; the answer is up to five documents, each with a "
            "quote. A quote that shares no six-word run with the text semlith extracted from the document it names "
            "was written, not read.\n\n"
            + table(["model", "arm", "runs", "questions", "found", "cost / session", "sessions calling semlith",
                     "top quotes not in the named document"], body)
            + "\n\n`python3 bench/scorecard/documents/agent_round.py run g,s,sx && python3 "
            "bench/scorecard/documents/agent_round.py score`")


def ledger(directory):
    doc = json.load(open(os.path.join(directory, "savings.json")))
    t = doc["total"]
    return (f"### Savings in real sessions\n\nThe owner's own ledger across {t['stores']} stores, benchmark stores "
            f"excluded: {t['retrievals']:,} retrievals, {t['coverage']} % credited ({'/'.join(t['tiers'])}). The "
            f"excerpts returned were {t['excerpt_tokens']:,} tokens; the whole files they came from, "
            f"{t['whole_tokens']:,} — {t['ratio']}x across all stores, {t['median_store_ratio']}x for the median "
            "store. That comparison is an upper bound on what reading would have cost, not a measurement of it; "
            "the agent table is the head-to-head.\n\n`python3 bench/scorecard/ledger_savings.py`")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--swe", default=newest("swe"))
    ap.add_argument("--coderag", default=newest("coderag"))
    ap.add_argument("--repobench", default=newest("repobench"))
    ap.add_argument("--competitors", default=os.path.join(HOME, "competitors", "score.json"))
    ap.add_argument("--agent", default=os.path.join(HOME, "agent", "corpus-round.json"))
    ap.add_argument("--ledger", default=os.path.dirname(sorted(glob.glob(os.path.join(RESULTS, "ledger", "*", "savings.json")))[-1])
                    if glob.glob(os.path.join(RESULTS, "ledger", "*", "savings.json")) else None)
    ap.add_argument("--documents", default=newest_documents("score.json"))
    ap.add_argument("--readme", help="write the tables between this file's scorecard:begin/end markers")
    a = ap.parse_args()
    parts = []
    for fn, arg in ((swe, a.swe), (coderag, a.coderag), (repobench, a.repobench), (competitors, a.competitors),
                    (agent, a.agent), (documents, a.documents), (ledger, a.ledger)):
        if arg and os.path.exists(arg):
            parts.append(fn(arg))
        else:
            parts.append(f"<!-- {fn.__name__}: no run found -->")
    out = "\n\n".join(parts)
    if not a.readme:
        print(out)
        return
    begin, end = "<!-- scorecard:begin -->", "<!-- scorecard:end -->"
    text = open(a.readme).read()
    head, rest = text.split(begin, 1)
    _, tail = rest.split(end, 1)
    with open(a.readme, "w") as f:
        f.write(head + begin + "\n" + out + "\n" + end + tail)


if __name__ == "__main__":
    main()
