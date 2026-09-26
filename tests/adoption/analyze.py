#!/usr/bin/env python3
"""Score a bench run: who did the looking, what it cost, how much it read.

A lookup is a call that goes looking for code: a semlith MCP tool, or the raw
habit it exists to replace -- Read, Grep, Glob, or a Bash command running grep,
rg, find, cat, sed, head, tail or awk. Housekeeping (ToolSearch, Skill,
TodoWrite, semlith_stats) is neither. Subagents' calls count: they are in the
same stream.

    python3 tests/adoption/analyze.py --runs tests/adoption/runs/latest

Writes scored.json into the run directory (judge.py reads it) and prints the
figures for the twelve original prompts, then for every prompt in the run, then
the gate lines this script can decide. Quality is judge.py's.

The dollar figures are `total_cost_usd` from `claude -p`: what the session
would cost at API prices. Under a claude.ai subscription nothing is charged;
the session spends usage quota instead.
"""

from __future__ import annotations

import argparse
import json
import re
import statistics as st
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from bench import ORIENTATION, ORIGINAL, PROMPTS  # noqa: E402

LEAD = r"(^|[|;&(]\s*|\s)"
RAW_BASH = re.compile(LEAD + r"(e?grep|rg|find|cat|sed|head|tail|awk)\b")
LISTING = re.compile(LEAD + r"(ls\b[^|;&]*\s(-\w*R\w*|--recursive)\b|tree\b|find\b[^|;&]*\s-(i?name|type)\b)")
RAW_TOOLS = {"Read", "Grep", "Glob"}
HOUSE_SEMLITH = ("semlith_stats", "semlith_languages")


def kind(name: str, inp: dict) -> str:
    if name.startswith("mcp__") and "semlith" in name:
        return "house" if name.endswith(HOUSE_SEMLITH) else "semlith"
    if name in RAW_TOOLS:
        return "raw"
    if name == "Bash" and RAW_BASH.search(inp.get("command", "")):
        return "raw"
    return "other"


def parse(f: Path):
    """Every tool call in the session, with the size of what it returned."""
    calls, results, res, mcp = [], {}, None, []
    for line in f.open(errors="replace"):
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if e.get("type") == "system" and e.get("subtype") == "init" and not mcp:
            mcp = e.get("mcp_servers") or []
        for c in (e.get("message") or {}).get("content") or []:
            if not isinstance(c, dict):
                continue
            if c.get("type") == "tool_use":
                inp = c.get("input") or {}
                calls.append({"id": c.get("id"), "name": c.get("name", ""), "input": inp,
                              "sub": bool(e.get("parent_tool_use_id")), "kind": kind(c.get("name", ""), inp)})
            elif c.get("type") == "tool_result":
                body = c.get("content")
                results[c.get("tool_use_id")] = (len(body if isinstance(body, str) else json.dumps(body)),
                                                 bool(c.get("is_error")))
        if e.get("type") == "result":
            res = e
        if e.get("type") == "bench_timeout":
            res = res or {"timeout": True}
    for c in calls:
        c["chars"], c["err"] = results.get(c["id"], (0, False))
    return mcp, calls, res


def score(f: Path) -> dict:
    arm, pid, rep = f.stem.split("__")
    mcp, calls, res = parse(f)
    sem = [c for c in calls if c["kind"] == "semlith"]
    raw = [c for c in calls if c["kind"] == "raw"]
    look = [c for c in calls if c["kind"] in ("semlith", "raw")]
    res = res or {}
    return {
        "arm": arm, "pid": pid, "rep": rep,
        "mcp": [(s.get("name"), s.get("status")) for s in mcp],
        "first": look[0]["kind"] if look else "none",
        "sem": len(sem), "raw": len(raw),
        "sem_chars": sum(c["chars"] for c in sem), "raw_chars": sum(c["chars"] for c in raw),
        "sem_err": sum(c["err"] for c in sem),
        "listings": sum(c["name"] == "Bash" and bool(LISTING.search(c["input"].get("command", ""))) for c in calls),
        "tree_view": any(c["name"].endswith("semlith_files") and c["input"].get("tree") in (True, "true")
                         for c in calls),
        "sem_tools": dict(Counter(c["name"].split("__")[-1] for c in sem)),
        "raw_tools": dict(Counter(c["input"].get("command", "").split()[0] if c["name"] == "Bash" and
                                  c["input"].get("command", "").split() else c["name"] for c in raw)),
        "cost": res.get("total_cost_usd"), "ms": res.get("duration_ms"), "turns": res.get("num_turns"),
        "answer": res.get("result") or "", "timeout": bool(res.get("timeout")) or not res,
    }


def pct(a: float, b: float) -> str:
    return f"{100 * a / b:.0f} %" if b else "-"


def median(xs) -> float:
    xs = list(xs)
    return st.median(xs) if xs else 0.0


def figures(rows: list[dict]) -> dict:
    """One arm's figures over finished sessions."""
    R = [r for r in rows if not r["timeout"]]
    S, W = sum(r["sem"] for r in R), sum(r["raw"] for r in R)
    return {
        "n": len(R), "timeouts": len(rows) - len(R),
        "share": S / (S + W) if S + W else 0.0, "S": S, "W": W,
        "first_sem": sum(r["first"] == "semlith" for r in R),
        "cost_mean": st.mean(r["cost"] or 0 for r in R) if R else 0.0,
        "cost_median": median(r["cost"] or 0 for r in R),
        "sem_out": median(r["sem_chars"] for r in R),
        "raw_out": median(r["raw_chars"] for r in R),
        "look_out": median(r["sem_chars"] + r["raw_chars"] for r in R),
        "listing_sessions": sum(r["listings"] > 0 for r in R),
    }


def table(title: str, by: dict[str, list[dict]], order: list[str]) -> dict[str, dict]:
    print(f"\n{title}")
    print(f"  {'arm':16} {'n':>3} {'share':>6} {'sem/raw':>9} {'1st=sem':>7} {'$/run':>6} {'$ med':>6} "
          f"{'sem out':>8} {'raw out':>8} {'look out':>8} {'ls -R etc':>9}")
    out = {}
    for arm in order:
        f = out[arm] = figures(by[arm])
        if not f["n"]:
            print(f"  {arm:16} no finished sessions ({f['timeouts']} timed out)")
            continue
        print(f"  {arm:16} {f['n']:>3} {pct(f['S'], f['S'] + f['W']):>6} {f['S']:>4}/{f['W']:<4} "
              f"{pct(f['first_sem'], f['n']):>7} {f['cost_mean']:>6.3f} {f['cost_median']:>6.3f} "
              f"{f['sem_out']:>8.0f} {f['raw_out']:>8.0f} {f['look_out']:>8.0f} "
              f"{f['listing_sessions']:>3} ({pct(f['listing_sessions'], f['n'])})"
              + (f"  [{f['timeouts']} timed out]" if f["timeouts"] else ""))
    return out


def gate(name: str, ok: bool, detail: str) -> bool:
    print(f"  {'PASS' if ok else 'FAIL'}  {name}: {detail}")
    return ok


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", default=str(HERE / "runs/latest"), help="run directory bench.py wrote")
    parser.add_argument("--arms", help="comma list to restrict to")
    args = parser.parse_args()

    runs = Path(args.runs)
    rows = [score(f) for f in sorted(runs.glob("*__*__r*.jsonl"))]
    if args.arms:
        rows = [r for r in rows if r["arm"] in args.arms.split(",")]
    if not rows:
        print(f"no sessions in {runs}", file=sys.stderr)
        return 2
    (runs / "scored.json").write_text(json.dumps(rows, indent=1) + "\n")

    by: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        by[r["arm"]].append(r)
    order = [a for a in ("off", "setup", "alwaysload-only", "hard") if a in by] + sorted(a for a in by if a not in
                                                                                       ("off", "setup", "alwaysload-only", "hard"))
    print(f"{len(rows)} sessions in {runs}")
    print("share = semlith lookups / all lookups, pooled. $ = total_cost_usd, an API-price equivalent, not a charge.")
    print("out = median characters of lookup results per session. 'ls -R etc' = sessions with a recursive listing.")

    orig = {a: [r for r in by[a] if r["pid"] in ORIGINAL] for a in order}
    table(f"the {len(ORIGINAL)} original prompts", orig, order)
    every = table(f"all {len({r['pid'] for r in rows})} prompts in the run", by, order)

    print(f"\norientation ({ORIENTATION}): semlith_files with tree: true, per repeat")
    for arm in order:
        reps = sorted((r["rep"], r["tree_view"]) for r in by[arm] if r["pid"] == ORIENTATION)
        print(f"  {arm:16} " + (", ".join(f"{rep} {'yes' if tv else 'no'}" for rep, tv in reps) or "-"))

    print("\nper prompt: semlith/raw lookups, pooled over repeats")
    print(f"  {'prompt':16}" + "".join(f"{a:>17}" for a in order))
    for pid in PROMPTS:
        if not any(r["pid"] == pid for r in rows):
            continue
        cells = []
        for arm in order:
            rr = [r for r in by[arm] if r["pid"] == pid]
            cells.append(f"{sum(r['sem'] for r in rr)}/{sum(r['raw'] for r in rr)}" if rr else "-")
        print(f"  {pid:16}" + "".join(f"{c:>17}" for c in cells))

    for label, key in (("semlith tools", "sem_tools"), ("raw tools", "raw_tools")):
        print(f"\n{label} used:")
        for arm in order:
            total: Counter = Counter()
            for r in by[arm]:
                total.update(r[key])
            print(f"  {arm:16} {dict(total.most_common(10))}")

    bad = sorted({(r["arm"], str(r["mcp"])) for r in rows if r["arm"] != "off"
                  and not any(s == "connected" for _, s in r["mcp"])})
    if bad:
        print("\nWARNING the semlith server was not connected in:", bad)

    if "off" in every and "setup" in every and every["setup"]["n"] and every["off"]["n"]:
        s, o = every["setup"], every["off"]
        print("\ngate (setup against off, every prompt in the run; quality is judge.py's)")
        tree = [r["tree_view"] for r in by["setup"] if r["pid"] == ORIENTATION]
        results = [
            gate("semlith share >= 80 %", s["share"] >= 0.80, f"{s['S']}/{s['S'] + s['W']} ({pct(s['S'], s['S'] + s['W'])})"),
            gate("cost <= off + 10 %", s["cost_mean"] <= o["cost_mean"] * 1.10,
                 f"{s['cost_mean']:.3f} against {o['cost_mean']:.3f} $/run (API-price equivalent)"),
            gate("lookup output median <= off's", s["look_out"] <= o["look_out"],
                 f"{s['look_out']:.0f} against {o['look_out']:.0f} chars/session"),
            gate("recursive listings <= 10 % of sessions", s["listing_sessions"] <= 0.10 * s["n"],
                 f"{s['listing_sessions']}/{s['n']}"),
            gate("orientation uses the tree view every repeat", bool(tree) and all(tree),
                 f"{sum(tree)}/{len(tree)}"),
        ]
        print(f"  {sum(results)}/{len(results)} pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
