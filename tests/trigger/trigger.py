#!/usr/bin/env python3
"""Does the skill, or the hook, change what an agent reaches for first?

The release ships two things whose only justification is that an agent behaves
differently with them than without: an Agent Skill that teaches the tools exist,
and a PreToolUse hook that names the call at the moment the agent is about to
read a file whole. Neither can be argued from the code. This measures them.

Four arms -- none, skill, hook, both -- over a fixed subset of the retrieval
harness's questions, drawn with a recorded seed. For each question it runs a
headless Claude Code session and reads the FIRST tool call it makes: a semlith
tool is a hit, a whole-file read or a repository-wide grep is a miss. The figure
is N of M per arm.

    python3 tests/trigger/trigger.py --store <dir> --questions 20 --runs 3

Everything runs against a redirected HOME so no arm can read or write the
developer's own Claude Code configuration -- which is also what makes the arms
mean anything, since an arm with the developer's real skills installed is not
the "none" arm.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
QUESTIONS = REPO / "tests/fixtures/retrieval/questions.yaml"
SKILL = REPO / "docs/skill/SKILL.md"

# The arms, and what each one installs.
ARMS = ["none", "skill", "hook", "both"]

# A first call that is one of these is the agent reaching for semlith.
SEMLITH_TOOL = "semlith"
# A first call that is one of these is the habit the release exists to change.
RAW_TOOLS = {"Read", "Grep", "Glob", "Bash"}


def questions(limit: int, seed: int) -> list[dict]:
    """A stratified subset, drawn with a recorded seed.

    Stratified by shape so an arm is not measured on twenty identifier lookups,
    which every arm gets right, or twenty concept questions, which none does.
    """
    import yaml  # noqa: PLC0415 — only needed when the harness actually runs

    every = yaml.safe_load(QUESTIONS.read_text())["questions"]
    by_shape: dict[str, list[dict]] = {}
    for q in every:
        by_shape.setdefault(q.get("shape", "other"), []).append(q)

    rng = random.Random(seed)
    for group in by_shape.values():
        rng.shuffle(group)

    picked: list[dict] = []
    shapes = sorted(by_shape)
    while len(picked) < limit and any(by_shape[s] for s in shapes):
        for shape in shapes:
            if by_shape[shape] and len(picked) < limit:
                picked.append(by_shape[shape].pop())
    picked.sort(key=lambda q: q["id"])
    return picked


def arm_flags(arm: str, root: Path, binary: Path, store_home: Path) -> list[str]:
    """The flags that make one arm what it is.

    Nothing here writes into the developer's own Claude Code configuration, and
    nothing redirects it either. Both were tried first and both are wrong:
    writing is the thing 0.21.0's suite did to a developer's machine, and
    redirecting `HOME` or `CLAUDE_CONFIG_DIR` loses the login, so every arm
    scores zero for a reason that has nothing to do with the skill.

    So the skill arrives as a one-skill plugin directory and the hook as a
    settings file, both per-run flags. What that costs is worth stating: the
    developer's own plugins and skills are loaded in every arm, so these figures
    are not "an agent with nothing else installed". They are the same agent with
    and without semlith's two additions, which is the comparison being made.
    """
    where = root / arm
    where.mkdir(parents=True, exist_ok=True)
    flags: list[str] = []

    if arm in ("skill", "both"):
        plugin = where / "plugin"
        (plugin / ".claude-plugin").mkdir(parents=True, exist_ok=True)
        (plugin / "skills/semlith").mkdir(parents=True, exist_ok=True)
        shutil.copy2(SKILL, plugin / "skills/semlith/SKILL.md")
        (plugin / ".claude-plugin/plugin.json").write_text(
            json.dumps(
                {
                    "name": "semlith-skill",
                    "version": "0.24.0",
                    "description": "The semlith Agent Skill, as `semlith setup` installs it.",
                },
                indent=2,
            )
            + "\n"
        )
        flags += ["--plugin-dir", str(plugin)]

    settings = where / "settings.json"
    if arm in ("hook", "both"):
        settings.write_text(
            json.dumps(
                {
                    "hooks": {
                        "PreToolUse": [
                            {
                                "matcher": "Read|Grep",
                                "hooks": [{"type": "command", "command": f"{binary} hook"}],
                            }
                        ]
                    }
                },
                indent=2,
            )
            + "\n"
        )
    else:
        settings.write_text("{}\n")
    flags += ["--settings", str(settings)]

    mcp = where / "mcp.json"
    mcp.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "semlith": {
                        "command": str(binary),
                        "args": ["mcp"],
                        "env": {"SEMLITH_HOME": str(store_home)},
                    }
                }
            },
            indent=2,
        )
        + "\n"
    )
    flags += ["--mcp-config", str(mcp), "--strict-mcp-config"]
    return flags


def is_lookup(name: str) -> bool:
    """Whether this call is the session looking for code.

    A session opens with housekeeping -- discovering tools, loading a skill,
    writing a plan -- and none of that is a code lookup. What this measures is
    the first call that goes looking, which is either a semlith tool or the
    habit it exists to replace. Counting `ToolSearch` as the first move would
    score every arm the same and measure nothing.
    """
    return SEMLITH_TOOL in name or name.split("__")[-1] in RAW_TOOLS


def first_tool(events: str) -> str | None:
    """The first code lookup the session made, or None if it made none."""
    for line in events.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = event.get("message") or {}
        for block in message.get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                name = block.get("name") or ""
                if is_lookup(name):
                    return name
    return None


def asked(question: dict) -> str:
    """The question as a person would type it.

    The harness's rows are shaped for the tool each one exercises: a search row
    carries a `query`, a symbol row a `name`, a path row a `from` and a `to`.
    What this measures is what an agent reaches for first, so every shape has to
    become one English sentence.
    """
    if "query" in question:
        return str(question["query"])
    if "name" in question:
        return f"where is {question['name']} defined, and what calls it"
    if "from" in question and "to" in question:
        return f"how does {question['from']} reach {question['to']}"
    if "target" in question:
        return f"what is {question['target']}"
    return str(question["id"]).replace("-", " ")


def ask(question: dict, flags: list[str], store_home: Path, cwd: Path, timeout: int):
    """One headless session, and the first tool it reached for."""
    prompt = (
        f"In this repository, answer: {asked(question)}. "
        "Find the relevant code first, then answer in one sentence."
    )
    env = dict(os.environ)
    env["SEMLITH_HOME"] = str(store_home)
    # This runs inside a Claude Code session of its own, and these two make the
    # child think it is that session.
    env.pop("CLAUDE_CODE_SSE_PORT", None)
    env.pop("CLAUDECODE", None)
    try:
        out = subprocess.run(
            [
                "claude",
                "-p",
                prompt,
                "--output-format",
                "stream-json",
                "--verbose",
                "--allowedTools",
                "Read,Grep,Glob,mcp__semlith__semlith_search,mcp__semlith__semlith_brief,"
                "mcp__semlith__semlith_read,mcp__semlith__semlith_symbol,"
                "mcp__semlith__semlith_neighbors,mcp__semlith__semlith_files",
                "--permission-mode",
                "acceptEdits",
                *flags,
            ],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return None, "timeout"
    return first_tool(out.stdout), (out.stderr or "").strip()[-200:]


def classify(tool: str | None) -> str:
    if tool is None:
        return "none"
    if SEMLITH_TOOL in tool:
        return "semlith"
    base = tool.split("__")[-1]
    return "raw" if base in RAW_TOOLS else "other"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--store", required=True, help="SEMLITH_HOME holding the store to search")
    parser.add_argument("--binary", default=str(REPO / "target/release/semlith"))
    parser.add_argument("--cwd", default=str(REPO), help="where the agent runs")
    parser.add_argument("--questions", type=int, default=20)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--seed", type=int, default=240)
    parser.add_argument("--arms", default=",".join(ARMS))
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--out", default="trigger-out")
    args = parser.parse_args()

    binary = Path(args.binary).resolve()
    if not binary.exists():
        print(f"no binary at {binary}", file=sys.stderr)
        return 2

    picked = questions(args.questions, args.seed)
    arms = [a for a in args.arms.split(",") if a]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    print(f"seed {args.seed} · {len(picked)} questions · arms {', '.join(arms)} · {args.runs} run(s)")
    print("questions: " + ", ".join(q["id"] for q in picked))

    results: dict[str, list[int]] = {arm: [] for arm in arms}
    detail: list[dict] = []
    with tempfile.TemporaryDirectory(prefix="semlith-trigger-") as tmp:
        root = Path(tmp)
        for run in range(args.runs):
            for arm in arms:
                flags = arm_flags(arm, root / f"run{run}", binary, Path(args.store))
                hits = 0
                for q in picked:
                    tool, err = ask(q, flags, Path(args.store), Path(args.cwd), args.timeout)
                    verdict = classify(tool)
                    hits += verdict == "semlith"
                    detail.append(
                        {"run": run, "arm": arm, "id": q["id"], "tool": tool, "verdict": verdict, "err": err}
                    )
                    print(f"  run{run} {arm:5} {q['id']:38} {tool or '—'} · {verdict}", flush=True)
                results[arm].append(hits)
                print(f"  run{run} {arm:5} → {hits} of {len(picked)}", flush=True)

    print("\n  trigger check · first code lookup is a semlith tool")
    for arm in arms:
        runs = sorted(results[arm])
        median = runs[len(runs) // 2] if runs else 0
        spread = f"{runs[0]}–{runs[-1]}" if len(runs) > 1 else str(median)
        print(f"    {arm:5} {median} of {len(picked)}   (runs {spread})")

    (out / "trigger.json").write_text(
        json.dumps(
            {
                "seed": args.seed,
                "questions": [q["id"] for q in picked],
                "runs": args.runs,
                "results": results,
                "detail": detail,
            },
            indent=2,
        )
        + "\n"
    )
    print(f"\n  written to {out / 'trigger.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
