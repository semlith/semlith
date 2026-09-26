#!/usr/bin/env python3
"""Blind grading of the answers, and the quality half of the gate.

For each prompt and repeat, every arm's answer is shuffled and relabelled A1,
A2, ... so the grader cannot tell which arm wrote which, then graded 0-10
against the reference key in refs.yaml by a separate headless `claude -p` with
no tools, no MCP servers, no CLAUDE.md and no memory. Grades are cached in the
run directory, so a second run only grades what is new.

    python3 tests/adoption/verify.py            # the keys still match the tree
    python3 tests/adoption/analyze.py --runs <dir>
    python3 tests/adoption/judge.py --runs <dir>

The difference setup - off is taken per (prompt, repeat) pair, and its 95 %
interval comes from resampling those pairs.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import statistics as st
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from bench import CLEAN_ENV, CROSS, ORIGINAL, PARENT_ENV, PROMPTS  # noqa: E402

GRADE = """You are grading answers to a question about a codebase (the semlith Rust repository, and for some
questions its sibling repositories semlith-cloud and infra). Grade each answer 0-10 against the reference
key, written by someone who verified the code. 10 = everything important in the key, correct file:line
(within ~15 lines), no wrong claims. Deduct for missing key items, wrong locations or wrong claims. Do not
reward length. Extra correct detail beyond the key is fine but earns little.

Question: {question}

Reference key: {key}

{answers}

Reply with ONLY a JSON object: {{"A1": {{"score": n, "note": "<=20 words"}}, ...}} covering every answer label."""


def grade(pid: str, rep: str, rows: list[dict], key: str, args) -> dict:
    answers = [r for r in rows if r["pid"] == pid and r["rep"] == rep and r["answer"]]
    random.Random(f"{pid}|{rep}").shuffle(answers)
    labels = {f"A{i + 1}": r["arm"] for i, r in enumerate(answers)}
    body = "\n\n".join(f"### Answer {label}\n{r['answer'][:6000]}" for label, r in zip(labels, answers))
    prompt = GRADE.format(question=PROMPTS[pid], key=key, answers=body)
    env = {k: v for k, v in os.environ.items() if k not in PARENT_ENV}
    env.update(CLEAN_ENV)
    cmd = ["claude", "-p", prompt, "--output-format", "json", "--setting-sources", "project,local",
           "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--tools", ""]
    if args.model:
        cmd += ["--model", args.model]
    err = ""
    for _ in range(3):
        done = subprocess.run(cmd, capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL,
                              cwd=tempfile.gettempdir(), timeout=600)
        try:
            reply = json.loads(done.stdout)
            text = reply["result"]
            graded = json.loads(text[text.index("{"): text.rindex("}") + 1])
            out = {labels[k]: v for k, v in graded.items() if k in labels}
            out["_arms"] = sorted(labels.values())
            out["_cost"] = reply.get("total_cost_usd")
            return out
        except (ValueError, KeyError, TypeError) as e:  # malformed output: ask again
            err = f"{e} {done.stdout[:200]} {done.stderr[:200]}"
    return {"_error": err, "_arms": []}


def interval(diffs: list[float], rounds: int) -> tuple[float, float]:
    rng = random.Random(0)
    means = sorted(st.mean(rng.choices(diffs, k=len(diffs))) for _ in range(rounds))
    return means[int(0.025 * rounds)], means[int(0.975 * rounds) - 1]


def report(rows: list[dict], judged: dict, rounds: int) -> None:
    score = {}
    for cell, res in judged.items():
        pid, rep = cell.split("|")
        for arm, v in res.items():
            if not arm.startswith("_") and isinstance(v, dict) and isinstance(v.get("score"), (int, float)):
                score[(arm, pid, rep)] = float(v["score"])
    arms = sorted({a for a, _, _ in score}, key=lambda a: (a != "off", a != "setup", a))
    cost = sum(r.get("_cost") or 0 for r in judged.values())

    def mean(arm, pids):
        qs = [q for (a, p, _), q in score.items() if a == arm and p in pids]
        return (st.mean(qs), len(qs)) if qs else (None, 0)

    print(f"quality, 0-10, blind ({len(judged)} gradings; judge cost {cost:.2f} $ API-price equivalent)")
    print(f"  {'arm':16} {'all':>10} {'original':>10} {'cross':>10}")
    for arm in arms:
        cells = []
        for pids in (list(PROMPTS), ORIGINAL, CROSS):
            m, n = mean(arm, pids)
            cells.append(f"{m:.2f} ({n})" if n else "-")
        print(f"  {arm:16} " + " ".join(f"{c:>10}" for c in cells))

    print(f"\ndifference against off, per (prompt, repeat) pair, 95 % bootstrap interval ({rounds} rounds)")
    for arm in arms:
        if arm == "off":
            continue
        for label, pids in (("all", list(PROMPTS)), ("original", ORIGINAL)):
            diffs = [q - score[("off", p, r)] for (a, p, r), q in score.items()
                     if a == arm and p in pids and ("off", p, r) in score]
            if diffs:
                lo, hi = interval(diffs, rounds)
                print(f"  {arm:16} {label:9} {st.mean(diffs):+.2f} ({lo:+.2f} .. {hi:+.2f}), {len(diffs)} pairs")

    print("\nper prompt, mean over repeats")
    print(f"  {'prompt':16}" + "".join(f"{a:>10}" for a in arms))
    for pid in PROMPTS:
        cells = [mean(a, [pid])[0] for a in arms]
        if any(c is not None for c in cells):
            print(f"  {pid:16}" + "".join(f"{c:>10.1f}" if c is not None else f"{'-':>10}" for c in cells))

    diffs = [q - score[("off", p, r)] for (a, p, r), q in score.items() if a == "setup" and ("off", p, r) in score]
    if diffs:
        lo, hi = interval(diffs, rounds)
        d = st.mean(diffs)
        print("\ngate (setup against off; the rest is analyze.py's)")
        print(f"  {'PASS' if d >= -0.3 else 'FAIL'}  quality >= off - 0.3: {d:+.2f} (95 % {lo:+.2f} .. {hi:+.2f})")
        s, o = mean("setup", CROSS)[0], mean("off", CROSS)[0]
        if s is not None and o is not None:
            print(f"  {'PASS' if s > o else 'FAIL'}  cross-store quality beats off: {s:.2f} against {o:.2f}")
        else:
            print("  -     cross-store quality: no cross-store prompts graded")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--runs", default=str(HERE / "runs/latest"), help="run directory analyze.py scored")
    parser.add_argument("--refs", default=str(HERE / "refs.yaml"))
    parser.add_argument("--parallel", type=int, default=4)
    parser.add_argument("--model", help="passed to claude --model for the grader")
    parser.add_argument("--rounds", type=int, default=10000, help="bootstrap resamples")
    parser.add_argument("--report-only", action="store_true", help="grade nothing; print from the cache")
    args = parser.parse_args()

    runs = Path(args.runs)
    scored = runs / "scored.json"
    if not scored.exists():
        print(f"no {scored}: run analyze.py --runs {runs} first", file=sys.stderr)
        return 2
    rows = json.loads(scored.read_text())
    keys = {pid: ref["key"] for pid, ref in yaml.safe_load(Path(args.refs).read_text())["refs"].items()}
    cache = runs / "judged.json"
    judged = json.loads(cache.read_text()) if cache.exists() else {}

    if not args.report_only:
        cells = sorted({(r["pid"], r["rep"]) for r in rows if r["pid"] in keys})
        todo = []
        for pid, rep in cells:
            arms = sorted(r["arm"] for r in rows if r["pid"] == pid and r["rep"] == rep and r["answer"])
            if judged.get(f"{pid}|{rep}", {}).get("_arms") != arms:
                todo.append((pid, rep))
        print(f"{len(todo)} of {len(cells)} (prompt, repeat) cells to grade", flush=True)
        with ThreadPoolExecutor(args.parallel) as ex:
            for (pid, rep), res in zip(todo, ex.map(lambda c: grade(c[0], c[1], rows, keys[c[0]], args), todo)):
                judged[f"{pid}|{rep}"] = res
                cache.write_text(json.dumps(judged, indent=1) + "\n")
                shown = {k: v.get("score") for k, v in res.items() if not k.startswith("_")} or res.get("_error")
                print(f"  {pid} {rep} {shown}", flush=True)
    report(rows, judged, args.rounds)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
