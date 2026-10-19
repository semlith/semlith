#!/usr/bin/env python3
"""Agent round on the document questions: headless Claude Code in SCORECARD_HOME/documents/files, one session per
(question, arm, run).

    python3 bench/scorecard/documents/agent_round.py run g,s,sx [--model opus] [--runs N] [--ids fmt-001,fmt-002]
    python3 bench/scorecard/documents/agent_round.py score [--out DIR]

Arms (the code round's flags, `agent/run.py`, isolated with per-run flags only, never HOME or CLAUDE_CONFIG_DIR):
  g   Grep, Glob and Read; no MCP server; no user CLAUDE.md, memory, plugins or hooks.
  s   g plus semlith as installed: `semlith mcp` (alwaysLoad), the skill and the hooks. Grep stays available.
  sx  s with Read the only built-in tool: every search goes through semlith.

semlith is SEMLITH_BIN serving the documents store, started through SCORECARD_HOME/documents/semlith, a wrapper
that sets the scratch SEMLITH_HOME, so neither the server nor the hook sees the user's own stores or daemon. The
skill is this repository's `docs/skill/SKILL.md`, the one the measured release ships.

A hit is a candidate in the gold file whose quote shares a 6-word run with the gold passage (documents.py's rule).
A top answer's quote that shares no 6-word run with the text semlith extracted from the document it names is
"not in the named document": the agent wrote the quote rather than read it.
"""
import argparse, json, os, re, shlex, shutil, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "agent"))
os.environ.setdefault("AGENT_SKILL", os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(HERE))), "docs", "skill", "SKILL.md"))
import documents as D
import run as R
from common import SEMLITH, median_spread, write_manifest

OUT = os.path.join(D.DOCS, "agent", "runs")
WRAPPER = os.path.join(D.DOCS, "semlith")
PROMPT = """The folder {root} holds documents of many formats (PDF, Word, PowerPoint, Excel, OpenDocument, EPUB, RTF, Jupyter notebooks, mailing-list archives, CSV). Work only inside it and do not modify anything.

Question: {query}

Find the document and the passage that answer it. End your reply with one line that starts with ANSWER: followed by a JSON list of at most 5 candidates, most likely first, each {{"path": "<path relative to the folder>", "quote": "<a verbatim quote of 15 to 40 words from the passage>"}}."""


def wrapper():
    binary = os.path.abspath(shutil.which(SEMLITH) or SEMLITH)
    env = " ".join(f"{k}={shlex.quote(os.environ[k])}" for k in ("SEMLITH_HOME", "SEMLITH_ACCEL", "SEMLITH_LEDGER"))
    with open(WRAPPER, "w") as f:
        f.write(f"#!/bin/sh\n{env} exec {shlex.quote(binary)} \"$@\"\n")
    os.chmod(WRAPPER, 0o755)
    return WRAPPER


def arm_flags(arm):
    os.makedirs(R.AGENT, exist_ok=True)
    if arm == "g":
        return R.arm_flags("g", None, None)
    s = R.arm_flags("s", wrapper(), "documents")
    return s if arm == "s" else ["Read" if f == R.TOOLS else f for f in s]


def run_one(arm, flags, model, k, q):
    path = os.path.join(OUT, model, arm, f"r{k}", f"{q['id']}.jsonl")
    if R._has(path, '"type":"result"'):
        return None
    env = {key: v for key, v in os.environ.items() if key not in ("CLAUDECODE", "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_ENTRYPOINT")}
    env.update({"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"})
    cmd = ["claude", "-p", PROMPT.format(root=D.ROOT, query=q["query"]), "--model", model, "--output-format", "stream-json",
           "--verbose", "--permission-mode", "bypassPermissions", "--max-budget-usd", "3", *flags]
    t = time.time()
    with open(path, "w") as f:
        try:
            subprocess.run(cmd, cwd=D.ROOT, env=env, stdout=f, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, timeout=1200)
        except subprocess.TimeoutExpired:
            f.write(json.dumps({"type": "bench_timeout"}) + "\n")
    return f"{model} {arm} r{k} {q['id']} {time.time() - t:.0f}s"


def run(a):
    qs = D.questions()
    if a.ids:
        qs = [q for q in qs if q["id"] in a.ids.split(",")]
    jobs = []
    for arm in a.arms.split(","):
        flags = arm_flags(arm)
        for k in range(a.runs):
            os.makedirs(os.path.join(OUT, a.model, arm, f"r{k}"), exist_ok=True)
            jobs += [(arm, flags, k, q) for q in qs]
    par = int(os.environ.get("AGENT_PAR", "3"))
    print(f"{len(jobs)} sessions, par {par}, model {a.model}, semlith {SEMLITH}", flush=True)
    with ThreadPoolExecutor(par) as ex:
        for msg in ex.map(lambda j: run_one(j[0], j[1], a.model, j[2], j[3]), jobs):
            if msg:
                print(msg, flush=True)
    print("ALL DONE", flush=True)


def parse_answer(text):
    """The last ANSWER: line's JSON list of {path, quote}, or []."""
    m = None
    for m in re.finditer(r"ANSWER:\s*(\[.*?\])\s*$", text, re.S | re.M):
        pass
    try:
        items = json.loads(m.group(1)) if m else []
    except ValueError:
        return []
    return [i for i in items if isinstance(i, dict) and i.get("path")]


def names(answer_path, path):
    """Whether an answer's path names the root-relative `path`."""
    p = D.relp(str(answer_path).strip())
    return p.endswith(path) or path.endswith(p.lstrip("./"))


def rows():
    qs = {q["id"]: q for q in D.questions()}
    gold = D.gold_text()
    docs = [f"{d['format']}/{d['filename']}" for d in D.manifest()]
    out = []
    for model in sorted(os.listdir(OUT)) if os.path.isdir(OUT) else []:
        for arm in sorted(os.listdir(os.path.join(OUT, model))):
            for rd in sorted(os.listdir(os.path.join(OUT, model, arm))):
                d = os.path.join(OUT, model, arm, rd)
                for name in sorted(os.listdir(d)):
                    q = qs[name[:-6]]
                    s = R.session(os.path.join(d, name))
                    if not s["timeout"] and not s["text"] and not s["subtype"]:
                        continue  # still running
                    ans = parse_answer(s["text"])[:5]
                    g = D.shingles(gold[q["target"]])
                    named = [x for x in ans if names(x["path"], q["path"])]
                    top = ans[0] if ans else {}
                    doc = next((p for p in docs if top.get("quote") and names(top["path"], p)), None)
                    inside = D.in_document(doc, D.shingles(str(top["quote"]))) if doc else None
                    out.append({"model": model, "arm": arm, "run": int(rd[1:]), "id": q["id"], "ext": D.fmt(q["path"]),
                                "shape": q["shape"], "answered": bool(ans), "hit": int(any(D.shingles(str(x.get("quote", ""))) & g for x in named)),
                                "file_hit": int(bool(named)), "top1_file": int(bool(ans) and ans[0] in named),
                                "top_document": doc, "top_quote_in_document": inside,
                                "semlith_calls": sum("semlith" in t for t in s["tools"]), "n_tools": len(s["tools"]),
                                "cost": s["cost"], "tokens_in": s["tokens_in"], "error": s["error"], "timeout": s["timeout"]})
    return out


def score(a):
    rs = rows()
    table = []
    for model, arm in sorted({(r["model"], r["arm"]) for r in rs}):
        per = []
        for k in sorted({r["run"] for r in rs if (r["model"], r["arm"]) == (model, arm)}):
            g = [r for r in rs if (r["model"], r["arm"], r["run"]) == (model, arm, k)]
            toks = sorted(r["tokens_in"] for r in g)
            checked = [r for r in g if r["top_quote_in_document"] is not None]
            per.append({"n": len(g), "hit": sum(r["hit"] for r in g), "file_hit": sum(r["file_hit"] for r in g),
                        "used_semlith": sum(r["semlith_calls"] > 0 for r in g), "semlith_calls": sum(r["semlith_calls"] for r in g),
                        "quotes_checked": len(checked), "quotes_not_in_document": sum(not r["top_quote_in_document"] for r in checked),
                        "cost_per_session": sum(r["cost"] for r in g) / len(g), "cost_total": sum(r["cost"] for r in g),
                        "tokens_in_median": toks[len(toks) // 2], "errors": sum(bool(r["error"]) for r in g),
                        "timeouts": sum(r["timeout"] for r in g)})
        row = {"model": model, "arm": arm, "runs": len(per), "n": [p["n"] for p in per]}
        for key in per[0]:
            if key != "n":
                row[key], row[key + "_spread"] = median_spread([p[key] for p in per])
        table.append(row)
    out = a.out or os.path.join(HERE, "runs", "agent-" + time.strftime("%Y%m%dT%H%M%S"))
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "agent-scores.jsonl"), "w") as f:
        for r in rs:
            f.write(json.dumps(r) + "\n")
    with open(os.path.join(out, "agent.json"), "w") as f:
        json.dump(table, f, indent=1)
    write_manifest(out, {"bench": "documents-agent", "store": D.STORE_DIR, "root": D.ROOT, "skill": os.environ["AGENT_SKILL"],
                         "fetch_mismatches": D.fetch_status()})
    for r in table:
        print(f"{r['model']:6} {r['arm']:3} runs {r['runs']} n {max(r['n'])}  found {r['hit']}  file {r['file_hit']}  "
              f"used semlith {r['used_semlith']} ({r['semlith_calls']} calls)  quotes not in document "
              f"{r['quotes_not_in_document']}/{r['quotes_checked']}  ${r['cost_per_session']:.3f}/session  "
              f"${r['cost_total']:.2f} total  errors {r['errors']}  timeouts {r['timeouts']}")
    print(out)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["run", "score"])
    ap.add_argument("arms", nargs="?", default="g,s,sx")
    ap.add_argument("--model", default="opus")
    ap.add_argument("--runs", type=int, default=1, help="1, as the 2026-10-01 round")
    ap.add_argument("--ids", help="comma list of question ids")
    ap.add_argument("--out", help="where score writes (default bench/scorecard/documents/runs/agent-<time>)")
    a = ap.parse_args()
    run(a) if a.cmd == "run" else score(a)
