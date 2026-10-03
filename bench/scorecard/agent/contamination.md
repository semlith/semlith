# SWE-bench Lite cannot measure an agent's retrieval on Opus

Measured on 2026-10-03 with `run.py` in this directory. The evidence is in
`~/semlith-bench/scorecard/agent/` (`sample.json`, `runs/opus/g/r{0,1,2}/`, `rows.jsonl`, `score.json`).

## Setup

- 50 SWE-bench Lite instances, seed 20261003, spread across all 12 repositories.
- Each instance was a worktree checked out at its base commit.
- Arm `g`: Grep, Glob and Read only. No MCP server, no user CLAUDE.md, memory, plugins or hooks.
- The prompt gave the issue text and asked which source files would have to change to fix it.
- Model `claude-opus-5-5`, 3 runs, 150 sessions.

## Result

| measure | value |
|---|---|
| sessions | 150 (50 instances x 3 runs), 0 timeouts |
| any gold file at rank 1 | 149 / 150 |
| every gold file in the top 10 | 150 / 150 |
| sessions with zero tool calls that still named the gold file | 30 / 150 |
| sessions with exactly one Grep | 97 / 150 |
| sessions with 2 or more tool calls | 23 / 150 |
| cost | $3.90 total, $0.024 per session, median 11.5k input tokens |

The only miss at rank 1 was `sympy__sympy-13146`. In that session the gold file came second.

## Conclusion

- One run in five never opened the repository and still named the right file. In most of the rest, one Grep only confirmed a path the model already knew.
- This is recall from memory of a public benchmark, not retrieval.
- Every Lite instance has exactly one gold file, so the ceiling is easy to reach.
- So on this model, no retrieval tool can show an accuracy gain on SWE-bench Lite. Only cost and adoption could differ, and even those reflect memorisation more than search.
- The agent round therefore moves to the accuracy benchmark's held-out questions. Those were generated from the 70-repository corpus's code and were never published.

The semlith arm was never run on Lite, for two reasons:
- The round was stopped once the `g` result settled the question.
- Indexing the 50 worktrees into one store (19 django and 13 sympy copies, about 110k files) ran at about 36 chunks/s while four other bench indexes shared the Neural Engine. That pointed to a wait of many hours.
