# The trigger check

Two things in 0.24.0 exist only because an agent is supposed to behave
differently with them than without: the Agent Skill, which teaches that the
tools are there, and the `PreToolUse` hook, which names the call at the moment
the agent is about to read a file whole. Neither can be argued from the code.
This measures them.

Four arms over a fixed subset of the retrieval harness's questions, drawn with a
recorded seed and stratified by shape. Each question is a headless Claude Code
session, and what is read out of it is the **first code lookup** it makes — a
semlith tool, or the whole-file read or repository-wide grep the release exists
to replace. Housekeeping calls (`ToolSearch`, `Skill`, `TodoWrite`) are skipped:
counting those as the first move would score every arm the same and measure
nothing.

| Arm | What it has |
|---|---|
| `none` | Neither. |
| `skill` | The skill, as a one-run plugin directory. |
| `hook` | The hook, as a one-run settings file. |
| `both` | Both. |

## Running it

```sh
cargo build --release
semlith index <some corpus>           # with SEMLITH_HOME pointed somewhere scratch
python3 tests/trigger/trigger.py \
  --store <that SEMLITH_HOME> \
  --questions 20 --runs 3 --seed 240 \
  --out trigger-out
```

It needs `claude` on `PATH`, a logged-in Claude Code, and `pyyaml`. Each session
is a real API call: twenty questions across four arms for three runs is 240 of
them, so start with `--questions 2 --runs 1 --arms none,both` to check the
plumbing before spending the quota.

## What it does not touch

Nothing is written into the developer's own Claude Code configuration. The skill
arrives through `--plugin-dir` and the hook through `--settings`, both per-run
flags, and the MCP server through `--mcp-config --strict-mcp-config`.

Redirecting `HOME` or `CLAUDE_CONFIG_DIR` was the obvious way to isolate the
arms and it is the wrong one: the login does not survive it, so every arm scores
zero for a reason that has nothing to do with the skill. That is worth knowing
before anyone tries it again.

What the flags cost, stated rather than hidden: the developer's own plugins and
skills load in all four arms. These are not figures for an agent with nothing
else installed. They are the same agent with and without semlith's two
additions, which is the comparison being made.

## Reading the result

`trigger.json` holds the seed, the question ids, the per-run counts and a row
per session with the tool it reached for. The summary prints N of M per arm with
the spread across runs beside the median — the spread is the instrument's noise,
and a difference between arms smaller than it is not a difference.
