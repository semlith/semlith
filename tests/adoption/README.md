# The adoption benchmark

The trigger check (`tests/trigger`) reads one thing out of a session: the
first tool it reached for. This one reads the whole session. It answers two
questions about a real agent in this repository:

- **Does it use semlith?** Of every lookup a session makes, main thread and
  subagents alike, what share is a semlith tool rather than Read, Grep, Glob
  or a Bash `grep`, `rg`, `find`, `cat`, `sed`, `head`, `tail` or `awk`?
- **Is it better or cheaper for it?** Every answer is graded blind against a
  reference key, and every session's cost and lookup output is measured.

It was a research harness first (2026-09-25, 216 sessions, in
`live-project-files/semlith/docs/agent-adoption-bench/`). That run found the
shipped setup at 7 % share and forcing adoption costing 0.8 of quality. This is
that harness, ported to be the 0.30.0 release gate.

## Arms

| Arm | What it has |
|---|---|
| `off` | No semlith: no MCP server, no skill, no hook. |
| `setup` | Exactly what `semlith setup` writes for Claude Code: the MCP server entry (with `alwaysLoad: true`), the skill, the hooks and the `semlith-explorer` agent. |
| `alwaysload-only` | Optional. The `setup` arm's server entry and nothing else. |
| `hard` | Optional. What `semlith setup --mode hard` writes. |

`off` and `setup` are the gate; the other two run only when named in `--arms`.

The `setup` arm is not a hand-copied stanza. `bench.py` runs `<binary> setup
--yes --airgap --no-service` with `HOME` pointed at a throwaway directory,
`SEMLITH_NO_SERVICE=1` and nothing but `claude` on `PATH`, then lifts what it
wrote into per-run flags:

| setup wrote | the arm gets |
|---|---|
| `~/.claude.json` `mcpServers.semlith` | `--mcp-config` under the key `semlith_bench`, with `--strict-mcp-config` |
| `~/.claude/settings.json` `hooks` | `--settings` |
| `~/.claude/skills/semlith` | `--plugin-dir`, as `skills/semlith` |
| `~/.claude/agents/semlith-explorer.md` | `--plugin-dir`, as `agents/` |

The key is `semlith_bench` because a per-project disable of `semlith` in the
owner's `~/.claude.json` beats `--mcp-config`. The tools are therefore
`mcp__semlith_bench__*`; a hook matcher written for `mcp__semlith__` is
rewritten to the bench key and the rewrite is printed, and one that still
cannot match stops the run. A hook *command* that checks the tool name itself
is not rewritten. From a plugin directory the skill and the agent load
namespaced (`semlith-bench:semlith`), where a real install has them bare.
Anything setup did not write (no `alwaysLoad`, no explorer agent) is printed
as a warning before any session starts.

Every arm runs with `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`,
`CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` and `--setting-sources project,local`: no
CLAUDE.md, no memory, no user-level settings, plugins or hooks. The owner's
`~/.claude*` files are never read or written. Redirecting `HOME` for the
sessions themselves is the wrong isolation: it loses the login.

## Prompts

Sixteen, in `bench.py`:

- the twelve from the original benchmark (impact, locate, concept, trace,
  cross-language, literal sweep, a bug hunt, docs, a plan);
- `overview-src`, the orientation question a directory listing answers;
- three cross-store prompts, whose answers are outside the directory the
  agent runs in: which `semlith-cloud` code depends on the core's MCP reply
  format, what in `infra` deploys the cloud service, and what breaks across
  both repositories if `Hit` gains a field.

The cross-store prompts need the sibling repositories indexed into stores
named `semlith-cloud` and `infra`. The bench never creates them unless asked:

```sh
semlith index ../semlith-cloud --name semlith-cloud
semlith index ../infra --name infra
# or let bench.py do exactly that with the binary under test:
python3 tests/adoption/bench.py --index-cross-stores ...
```

If a daemon is running, restart it afterwards so it serves the new stores.

## The gate

Setup against off, pooled over two repeats of all sixteen prompts. It holds
open if any line fails.

| Criterion | Measured by |
|---|---|
| semlith share of lookups >= 80 %, pooled | `analyze.py` |
| blind quality >= off - 0.3, with a 95 % bootstrap interval over (prompt, repeat) pairs | `judge.py` |
| cost per session <= off + 10 % (mean `total_cost_usd`) | `analyze.py` |
| median lookup output per session (semlith plus raw result characters) <= off's | `analyze.py` |
| sessions with a recursive listing (`ls -R`, `tree`, `find -name/-type`) <= 10 % | `analyze.py` |
| the orientation prompt calls `semlith_files` with `tree: true`, both repeats | `analyze.py` |
| cross-store quality beats off | `judge.py` |

`analyze.py` also prints every figure for the twelve original prompts alone,
so the result can be set against the 2026-09-25 baseline.

## Running it

```sh
cargo build --release
# the stdio server forwards to a running daemon, so the daemon answering
# must be the binary under test: restart it from target/release first
python3 tests/adoption/verify.py                          # keys still match the tree
python3 tests/adoption/bench.py --binary target/release/semlith --dry-run
python3 tests/adoption/bench.py --binary target/release/semlith --out tests/adoption/runs/0.30.0
python3 tests/adoption/analyze.py --runs tests/adoption/runs/0.30.0
python3 tests/adoption/judge.py --runs tests/adoption/runs/0.30.0
```

`--dry-run` runs the sandboxed `semlith setup`, builds the arms, and prints
the flags and any warnings without starting a session. Start there, then with
`--prompts overview-src --repeats 1` to check the plumbing before spending the
quota. Other knobs: `--arms`, `--prompts` (ids, or `original`, `cross`,
`all`), `--repeats` (default 2), `--parallel` (default 4), `--model`,
`--timeout`. Finished sessions in `--out` are skipped, so an interrupted run
resumes; use a fresh `--out` per binary.

It needs `claude` on `PATH`, a logged-in Claude Code, and `pyyaml`.

## What it costs

Sixteen prompts, two repeats, two arms: 64 headless Opus sessions, plus one
grading call per (prompt, repeat), about 40 minutes at four at a time. Under a
claude.ai subscription that spends usage quota, not money. The dollar figures
every script prints are `total_cost_usd` from `claude -p`: what the same
sessions would cost at API prices, an equivalent and not a charge.

## Output

Everything goes under `tests/adoption/runs/`, which is gitignored: one
stream-json file per session, `manifest.json` (binary, version, the arms'
exact flags), `arms/` (the lifted files), `scored.json` and `judged.json`.
Run output is never committed; quote the figures in the release record
instead.

## Keeping the keys honest

`refs.yaml` holds one reference key per prompt and, under it, the `path:line`
facts the key rests on. `verify.py` checks each one against the current tree:
the cited line must still contain the cited text, or it reports the line it
moved to (MOVED) or that it is gone (GONE); count facts re-count, such as the
four `Hit` literals and the zero TODOs. Run it on the release tree before
judging, and update the key text for anything it reports. A key that has
drifted grades a correct answer down.
