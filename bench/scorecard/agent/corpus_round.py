#!/usr/bin/env python3
"""The agent round on the accuracy benchmark's 50 held-out questions (the 70-repository corpus), per (model, arm).

    python3 bench/scorecard/agent/corpus_round.py

Reads ~/semlith-bench/accuracy/agent/scores-038{N1..3,C,C2,C3,G,G2,G3}{,-haiku}.jsonl, one file per run written by
that harness's `agent.py score`. Arm g (tags N: "no semlith"; lower-case g tags would collide with G on a
case-insensitive filesystem) is Grep/Glob/Read only, s is the installed semlith (soft hook), and gate is s
with the hook in gate mode (`hook --strict`). Prints the table and writes it to
SCORECARD_HOME/agent/corpus-round.json. Each metric is the median of the runs, with the spread (max - min).
"""
import json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from common import HOME, median_spread

ACC = os.path.expanduser(os.environ.get("ACCURACY_AGENT", "~/semlith-bench/accuracy/agent"))
NAME = re.compile(r"^scores-038([NCG])(\d?)(-haiku)?\.jsonl$")
ARM = {"N": "g", "C": "s", "G": "gate"}


def has(run_dir, row, needle):
    p = os.path.join(run_dir, f"{row['arm']}__{row['id']}.jsonl")
    return os.path.exists(p) and needle in open(p, errors="replace").read()


def per_run(rows, run_dir):
    n = len(rows)
    hits = sum(r["hit"] for r in rows)
    cost = sum(r["cost"] or 0 for r in rows)
    toks = sorted(r["tokens_in"] or 0 for r in rows)
    return {"n": n, "hit": hits / n, "file_hit": sum(r["file_hit"] for r in rows) / n,
            "cost_per_session": cost / n, "cost_total": cost, "cost_per_correct": cost / hits if hits else None,
            "tokens_in_median": toks[n // 2], "used_semlith": sum(r["semlith_calls"] > 0 for r in rows),
            "zero_tools": sum(r["n_tools"] == 0 for r in rows), "timeouts": sum(has(run_dir, r, "bench_timeout") for r in rows),
            # A 429 (subscription limit) is not an answer; a valid run has none.
            "rate_limited": sum(has(run_dir, r, '"api_error_status":429') for r in rows)}


def main():
    groups = {}
    for name in sorted(os.listdir(ACC)):
        m = NAME.match(name)
        if not m:
            continue
        kind, num, haiku = m.groups()
        tag = f"-038{kind}{num}"
        rows = [json.loads(l) for l in open(os.path.join(ACC, name))]
        run_dir = os.path.join(ACC, "runs" + tag + (haiku or ""))
        groups.setdefault(("haiku" if haiku else "opus", ARM[kind]), []).append((tag, per_run(rows, run_dir)))
    table = []
    for (model, arm), runs in sorted(groups.items()):
        row = {"model": model, "arm": arm, "runs": len(runs), "tags": [t for t, _ in runs], "n": [r["n"] for _, r in runs]}
        for k in runs[0][1]:
            if k != "n":
                vals = [r[k] for _, r in runs if r[k] is not None]
                row[k], row[k + "_spread"] = median_spread(vals) if vals else (None, None)
        table.append(row)
    out = os.path.join(HOME, "agent", "corpus-round.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w") as f:
        json.dump(table, f, indent=1)
    keys = [("hit", 3), ("file_hit", 3), ("cost_per_session", 4), ("cost_per_correct", 4), ("tokens_in_median", 0),
            ("used_semlith", 0), ("zero_tools", 0), ("timeouts", 0), ("rate_limited", 0)]
    print(f"{'model':6} {'arm':5} runs  n  " + " ".join(f"{k:>17}" for k, _ in keys))
    for r in table:
        cells = ["-" if r[k] is None else f"{r[k]:.{p}f}±{r[k + '_spread']:.{p}f}" for k, p in keys]
        print(f"{r['model']:6} {r['arm']:5} {r['runs']:4} {max(r['n']):3} " + " ".join(f"{c:>17}" for c in cells))
    print(f"total cost ${sum(r['cost_total'] for _, rs in groups.items() for _, r in rs):.2f}; -> {out}")


if __name__ == "__main__":
    main()
