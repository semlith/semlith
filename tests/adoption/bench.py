#!/usr/bin/env python3
"""Does an agent use semlith when it has it, and are its answers better for it?

The trigger check (tests/trigger) reads one thing out of a session: the first
tool it reached for. This reads the whole session. Every prompt runs as a
headless `claude -p` in this repository, once per arm and repeat, and the
stream-json of the main thread and every subagent is kept for analyze.py
(who did the looking, what it cost) and judge.py (was the answer right).

The arms are built from what `semlith setup` writes, not from a hand-copied
stanza. setup runs against a throwaway HOME, with the login service and the
model download off and nothing but `claude` on PATH, and what it wrote for
Claude Code is lifted into per-run flags:

  ~/.claude.json mcpServers.semlith   -> --mcp-config, under the key semlith_bench
  ~/.claude/settings.json hooks       -> --settings
  ~/.claude/skills/semlith            -> --plugin-dir <dir>/skills/semlith
  ~/.claude/agents/semlith-explorer.md -> --plugin-dir <dir>/agents

    python3 tests/adoption/bench.py --binary target/release/semlith

The owner's own ~/.claude* files are never read or written. The server key is
not `semlith` because a per-project disable of `semlith` in ~/.claude.json
beats --mcp-config.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
# The folder holding this repository and its siblings.
LIVE = REPO.parent
CROSS_REPOS = {"semlith-cloud": LIVE / "semlith-cloud", "infra": LIVE / "infra"}

PROMPTS = {
    # The twelve from the 2026-09-25 benchmark, unchanged.
    "impact-sig": "What's impacted if I change the signature of `Fleet::search_preferring`? List every call site that would break.",
    "locate-const": "Where is the reciprocal rank fusion constant defined, what value does it use, and why that value?",
    "how-stale": "How does semlith decide that a search hit is stale, and what does it do to a stale hit's score?",
    "callers-brief": "What calls the brief function behind semlith_brief, across the CLI, the MCP server and the portal?",
    "trace-mcp": "Trace how a semlith_search MCP request travels from the JSON-RPC handler down to the vector index lookup. Give the chain of functions with file:line.",
    "crosslang": "Which portal JavaScript functions render a search hit row, and which Rust HTTP route supplies their data?",
    "hit-literals": "List every place in the Rust code that constructs a `Hit` struct literal.",
    "bug-startup": "The daemon log prints `semlith: watching — 367 files, 11667 chunks (0 indexed at startup, 0 unchanged)`. Find the code that prints this and tell me whether '0 indexed, 0 unchanged' for 367 files is a bug.",
    "docs-contract": "What does semlith promise publicly about the `score` field of a search result, and where is that written down?",
    "plan-confidence": "In this repo, who and where would things change if I want to add a confidence level to all searches done?",
    "concept-secrets": "How does semlith avoid indexing secrets like API keys or .env files? Point me to the code.",
    "grep-todo": "Find every TODO or FIXME comment in src/.",
    # Orientation: the question a listing answers, and the tree view exists for.
    "overview-src": "Give me an overview of how `src/` is organised and which files hold what",
    # Cross-store: the answer is outside the directory the agent runs in.
    "x-cloud-mcp": "Which code in the semlith-cloud repository depends on the format of the core crate's MCP replies? Name the files and functions with file:line.",
    "x-infra-deploy": "What in the infra repository deploys the Semlith Cloud service, and how does a change reach production?",
    "x-hit-field": "If the core `Hit` struct gains a new field, what breaks across both the semlith core repository and the semlith-cloud repository? Give file:line for each.",
}
ORIGINAL = list(PROMPTS)[:12]
ORIENTATION = "overview-src"
CROSS = ["x-cloud-mcp", "x-infra-deploy", "x-hit-field"]

SERVER_KEY = "semlith_bench"
# A tool name as the setup arm's session sees it. Every hook matcher that means
# semlith's tools has to match this, not `mcp__semlith__…`.
BENCH_TOOL = f"mcp__{SERVER_KEY}__semlith_search"

# No CLAUDE.md, no auto memory, no user-level settings, plugins or hooks.
CLEAN_ENV = {"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"}
CLEAN = ["--setting-sources", "project,local"]
# These make a child `claude` think it is the session that launched it.
PARENT_ENV = ("CLAUDECODE", "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_ENTRYPOINT")

# The arms that run `semlith setup`, and the flags each passes it.
SETUP_ARGS = {"setup": [], "hard": ["--mode", "hard"]}
ARMS = ["off", "setup", "alwaysload-only", "hard"]
DEFAULT_ARMS = ["off", "setup"]


def write_json(path: Path, value) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")
    return str(path)


def repoint(value, sandbox: Path, binary: Path):
    """Point every path setup wrote at the binary under test.

    setup names the executable that ran it, which is `binary` already; a copy
    under the sandbox's `.semlith/bin` would be gone by the time a session
    starts, so it is replaced, and any other sandbox path is an error.
    """
    if isinstance(value, str):
        value = value.replace(str(sandbox / ".semlith/bin/semlith"), str(binary))
        if str(sandbox) in value:
            sys.exit(f"setup wrote a path inside its throwaway HOME, which will not exist: {value}")
        return value
    if isinstance(value, list):
        return [repoint(v, sandbox, binary) for v in value]
    if isinstance(value, dict):
        return {k: repoint(v, sandbox, binary) for k, v in value.items()}
    return value


def rekey(hooks: dict) -> list[str]:
    """Make every matcher aimed at semlith's MCP tools match the bench key."""
    notes = []
    for event, entries in hooks.items():
        for entry in entries:
            matcher = entry.get("matcher") or ""
            if "mcp" not in matcher or re.search(matcher, BENCH_TOOL):
                continue
            renamed = matcher.replace("mcp__semlith__", f"mcp__{SERVER_KEY}__")
            if not re.search(renamed, BENCH_TOOL):
                sys.exit(f"{event} matcher {matcher!r} cannot match {BENCH_TOOL}; the arm would drop it")
            entry["matcher"] = renamed
            notes.append(f"{event} matcher {matcher!r} -> {renamed!r}")
    return notes


def lift(binary: Path, extra: list[str], plugin: Path) -> tuple[dict, dict, list[str]]:
    """Run `semlith setup` on a throwaway HOME; return its server entry and hooks.

    The skill and the explorer agent are copied into `plugin` as a one-run
    plugin. Guards, each for something setup would otherwise do to the
    machine: `--no-service` and SEMLITH_NO_SERVICE (a login service is not a
    file HOME redirects), `--airgap` (no 90 MB download per arm), and a PATH
    holding only `claude`, so no other client's CLI is run.
    """
    claude = shutil.which("claude")
    if not claude:
        sys.exit("no `claude` on PATH")
    with tempfile.TemporaryDirectory(prefix="semlith-adoption-setup-") as tmp:
        root = Path(tmp).resolve()
        home, bindir = root / "home", root / "path"
        home.mkdir()
        bindir.mkdir()
        (bindir / "claude").symlink_to(claude)
        env = {
            "HOME": str(home),
            "PATH": f"{bindir}:/usr/bin:/bin:/usr/sbin:/sbin",
            "SHELL": "/bin/zsh",
            "TERM": "dumb",
            "LANG": os.environ.get("LANG", "C.UTF-8"),
            "SEMLITH_NO_SERVICE": "1",
        }
        cmd = [str(binary), "setup", "--yes", "--airgap", "--no-service", *extra]
        done = subprocess.run(cmd, cwd=home, env=env, stdin=subprocess.DEVNULL,
                              capture_output=True, text=True, timeout=300)
        if done.returncode != 0:
            sys.exit(f"`{' '.join(cmd[1:])}` exited {done.returncode}:\n{(done.stdout + done.stderr)[-1500:]}")

        registered = json.loads((home / ".claude.json").read_text()).get("mcpServers") or {}
        if "semlith" not in registered:
            sys.exit("setup registered no `semlith` server in ~/.claude.json")
        server = repoint(registered["semlith"], home, binary)

        settings = home / ".claude/settings.json"
        hooks = json.loads(settings.read_text()).get("hooks", {}) if settings.exists() else {}
        hooks = repoint(hooks, home, binary)

        skill = home / ".claude/skills/semlith"
        if not skill.exists():
            sys.exit("setup linked no skill into ~/.claude/skills/semlith")
        shutil.rmtree(plugin, ignore_errors=True)
        shutil.copytree(skill.resolve(), plugin / "skills/semlith")
        notes = []
        agent = home / ".claude/agents/semlith-explorer.md"
        if agent.exists():
            (plugin / "agents").mkdir(parents=True)
            # The agent's tools name the server key `semlith`; the bench's
            # server is registered under another key, so the names follow it.
            text = agent.read_text(encoding="utf-8").replace("mcp__semlith__", "mcp__" + SERVER_KEY + "__")
            (plugin / "agents/semlith-explorer.md").write_text(text, encoding="utf-8")
        else:
            notes.append("WARNING setup wrote no ~/.claude/agents/semlith-explorer.md")
        write_json(plugin / ".claude-plugin/plugin.json",
                   {"name": "semlith-bench", "version": "0.0.0",
                    "description": "What `semlith setup` installed, as a one-run plugin."})
    if not server.get("alwaysLoad"):
        notes.append("WARNING the server entry setup wrote has no `alwaysLoad: true`")
    notes += rekey(hooks)
    return server, hooks, notes


def build_arms(names: list[str], binary: Path, where: Path) -> dict[str, dict]:
    """The flags that make each arm what it is, plus what was noticed building it."""
    arms: dict[str, dict] = {}
    lifted: dict[str, tuple] = {}

    def setup(name: str) -> tuple:
        if name not in lifted:
            lifted[name] = lift(binary, SETUP_ARGS[name], where / name / "plugin")
        return lifted[name]

    for name in names:
        d = where / name
        if name == "off":
            mcp = write_json(d / "mcp.json", {"mcpServers": {}})
            arms[name] = {"flags": CLEAN + ["--strict-mcp-config", "--mcp-config", mcp], "notes": []}
        elif name == "alwaysload-only":
            server, _, notes = setup("setup")
            mcp = write_json(d / "mcp.json", {"mcpServers": {SERVER_KEY: {**server, "alwaysLoad": True}}})
            arms[name] = {"flags": CLEAN + ["--strict-mcp-config", "--mcp-config", mcp],
                          "notes": []}
        else:
            server, hooks, notes = setup(name)
            mcp = write_json(d / "mcp.json", {"mcpServers": {SERVER_KEY: server}})
            settings = write_json(d / "settings.json", {"hooks": hooks})
            arms[name] = {"flags": CLEAN + ["--strict-mcp-config", "--mcp-config", mcp,
                                            "--settings", settings, "--plugin-dir", str(d / "plugin")],
                          "notes": notes}
    return arms


def registered_stores() -> set[str]:
    home = Path(os.environ.get("SEMLITH_HOME") or Path.home() / ".semlith")
    try:
        return set(json.loads((home / "registry.json").read_text())["stores"])
    except (OSError, ValueError, KeyError):
        return set()


def index_cross_stores(binary: Path) -> None:
    for name, path in CROSS_REPOS.items():
        print(f"indexing {path} into store {name}", flush=True)
        subprocess.run([str(binary), "index", str(path), "--name", name, "--quiet"], check=True)


def run(arm: str, flags: list[str], pid: str, rep: int, out: Path, args) -> str:
    """One headless session, its stream-json kept whole."""
    f = out / f"{arm}__{pid}__r{rep}.jsonl"
    if f.exists() and '"type":"result"' in f.read_text():
        return f"skip {f.name}"
    env = {k: v for k, v in os.environ.items() if k not in PARENT_ENV}
    env.update(CLEAN_ENV)
    cmd = ["claude", "-p", PROMPTS[pid], "--output-format", "stream-json", "--verbose",
           "--permission-mode", "bypassPermissions", "--disallowedTools", "Edit,Write,NotebookEdit", *flags]
    if args.model:
        cmd += ["--model", args.model]
    started = time.time()
    with open(f, "w") as fh:
        try:
            subprocess.run(cmd, cwd=args.cwd, env=env, stdout=fh, stderr=subprocess.STDOUT,
                           stdin=subprocess.DEVNULL, timeout=args.timeout)
        except subprocess.TimeoutExpired:
            fh.write(json.dumps({"type": "bench_timeout"}) + "\n")
    return f"done {f.name} {time.time() - started:.0f}s"


def pick_prompts(spec: str) -> list[str]:
    groups = {"all": list(PROMPTS), "original": ORIGINAL, "cross": CROSS}
    picked: list[str] = []
    for word in filter(None, spec.split(",")):
        for pid in groups.get(word, [word]):
            if pid not in PROMPTS:
                sys.exit(f"unknown prompt {pid!r}; known: {', '.join(PROMPTS)}")
            if pid not in picked:
                picked.append(pid)
    return picked


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", default=str(REPO / "target/release/semlith"),
                        help="the semlith binary the arms run `setup` with and serve MCP from")
    parser.add_argument("--arms", default=",".join(DEFAULT_ARMS),
                        help=f"comma list from {', '.join(ARMS)} (default: %(default)s)")
    parser.add_argument("--prompts", default="all",
                        help="comma list of prompt ids, or the groups all, original, cross (default: all)")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--parallel", type=int, default=4, help="sessions at once")
    parser.add_argument("--out", default=str(HERE / "runs/latest"),
                        help="run directory; finished sessions in it are skipped, so a run resumes (default: %(default)s)")
    parser.add_argument("--timeout", type=int, default=1200, help="seconds per session")
    parser.add_argument("--model", help="passed to claude --model; the account default when omitted")
    parser.add_argument("--cwd", default=str(REPO), help="where the agent runs (default: this repository)")
    parser.add_argument("--index-cross-stores", action="store_true",
                        help="index semlith-cloud and infra into stores of those names first (writes to your store home)")
    parser.add_argument("--dry-run", action="store_true", help="build the arms and print them; run no session")
    args = parser.parse_args()

    binary = Path(args.binary).resolve()
    if not binary.exists():
        print(f"no binary at {binary}", file=sys.stderr)
        return 2
    arms = [a for a in args.arms.split(",") if a]
    unknown = [a for a in arms if a not in ARMS]
    if unknown:
        print(f"unknown arm(s) {', '.join(unknown)}; known: {', '.join(ARMS)}", file=sys.stderr)
        return 2
    prompts = pick_prompts(args.prompts)

    if args.index_cross_stores:
        index_cross_stores(binary)
    missing = sorted(set(CROSS_REPOS) - registered_stores())
    if any(p in CROSS for p in prompts) and missing:
        print(f"no store named {', '.join(missing)}: pass --index-cross-stores, or leave the cross "
              f"prompts out with --prompts original,{ORIENTATION}", file=sys.stderr)
        return 2

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    version = subprocess.run([str(binary), "--version"], capture_output=True, text=True).stdout.strip()
    built = build_arms(arms, binary, out / "arms")
    (out / "manifest.json").write_text(json.dumps(
        {"binary": str(binary), "version": version, "cwd": args.cwd, "model": args.model,
         "prompts": prompts, "repeats": args.repeats, "arms": built}, indent=2) + "\n")

    print(f"{version} · arms {', '.join(arms)} · {len(prompts)} prompts · {args.repeats} repeat(s) → {out}")
    for name, arm in built.items():
        for note in arm["notes"]:
            print(f"  {name}: {note}")
    if args.dry_run:
        for name, arm in built.items():
            print(f"  {name}: claude -p <prompt> {' '.join(arm['flags'])}")
        return 0

    jobs = [(arm, pid, rep) for rep in range(args.repeats) for pid in prompts for arm in arms]
    print(f"{len(jobs)} sessions, {args.parallel} at once", flush=True)
    with ThreadPoolExecutor(args.parallel) as ex:
        for line in ex.map(lambda j: run(j[0], built[j[0]]["flags"], j[1], j[2], out, args), jobs):
            print(line, flush=True)
    print(f"all done; next: python3 {HERE / 'analyze.py'} --runs {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
