# The scorecard harness

Everything the README's Benchmarks section publishes is produced here. Each
number is the median of three runs, with its spread beside it when it is not
zero, and every run writes a `MANIFEST.json` naming its command, the semlith
version, the measured binary's SHA-256 (and, when `SEMLITH_BIN_COMMIT` names it,
the commit it was built from), the machine and the SHA-256 of every result file.

Nothing this harness downloads, clones, indexes or writes lives in the
repository. It all goes under `SCORECARD_HOME`, `~/semlith-bench/scorecard` by
default, and none of `bench/` is part of the published crate.

## What you need

- `semlith` on `PATH`, or `SEMLITH_BIN` pointing at the binary to measure.
- Python 3.11 and [uv](https://docs.astral.sh/uv/); the only dependency is
  `pyarrow`, pulled in by `uv run --with pyarrow`.
- `rg` for the grep baseline.
- For the competitor arms, the nine tools; `competitors/README.md` says where
  each comes from and how its adapter calls it. Sourcebot needs Docker.
- For the agent round, Claude Code signed in (`claude -p`).

## Downloads

`uv run --with pyarrow python bench/scorecard/fetch.py all` fetches, once each:

| What | From | Size |
|---|---|---|
| SWE-bench Lite and Verified | Hugging Face `princeton-nlp/SWE-bench_Lite`, `_Verified` | 1.2 MB, 2.1 MB |
| The 12 SWE-bench repositories, full history | GitHub | about 3 GB |
| CodeRAG-Bench task sets and corpora | Hugging Face `code-rag-bench/*` | about 16 MB |
| RepoEval function-level tasks and repositories | the URLs CodeRAG-Bench's own `retrieval/create/repoeval.py` uses | about 25 MB zipped |
| RepoBench-R test splits | Hugging Face `tianyang/repobench-r` | about 0.7 GB |

## The runs

| Benchmark | Command | Time on an M1 Air |
|---|---|---|
| SWE-bench retrieval | `uv run --with pyarrow python bench/scorecard/swe.py walk` then `... swe.py score <run dir>`; after the walk, `... swe.py check` compares a seeded three of its stores with `git ls-files` and a fresh index of the same checkout | many hours: every instance is checked out at its base commit and re-indexed incrementally |
| CodeRAG-Bench | `uv run --with pyarrow python bench/scorecard/coderag.py run` then `... score <run dir>` | about an hour, mostly indexing the 34 003 library pages |
| RepoBench-R | `uv run --with pyarrow python bench/scorecard/repobench.py run` then `... score <run dir>` | about 2 h for the default sample of 500 per configuration and level; `--sample 0` runs all 48 000 |
| Competitors | `python3.11 bench/scorecard/competitors_swe.py walk` then `... score`; to run the slow tools beside the fast ones, give them their own `--out DIR` with `SEMLITH_VECTOR_CACHE_MB=0` (otherwise semlith's first index of a repository there reuses the vectors the first walk cached, and the budget is no longer against a cold index), and score with `--also DIR=tool,tool` | depends on the tools; each is stopped where its indexing passes four times semlith's on the same instances |
| Savings in real sessions | `python3 bench/scorecard/ledger_savings.py` | seconds; reads this machine's own ledger |
| The README tables | `python3 bench/scorecard/report.py` | seconds |

Each run resumes where it stopped: results append one line per (instance, arm,
run) to `rows.jsonl`, and a line already there is not run again.

## What each arm is

- **semlith**: `semlith search` against a store of exactly the files the
  benchmark gives, through the CLI or one long-lived `semlith mcp` server.
- **grep then read** (SWE-bench): one `rg` pass counts the query's terms in
  every file, BM25 ranks the files, and they are read whole in that order —
  what an agent that greps and reads pays.
- **BM25** (CodeRAG-Bench, RepoBench-R): the same term weighting over the
  benchmark's own documents.
- **competitors**: `competitors/`, one adapter per tool, each calling that
  tool's own CLI or server as its documentation describes.

Tokens are counted at four bytes each over what an arm returns, in the order it
returns it, so a budget means the same thing for every arm.

## Known limits of the method

- **SWE-bench Lite cannot measure an agent on Opus.** Run with Grep, Glob and
  Read only, Opus named the gold file at the top in 149 of 150 sessions, 30 of
  them without a single tool call (`agent/contamination.md`). The retrieval
  scores above are unaffected — they rank what a tool returns, and no model is
  involved — but the agent round runs on questions generated from the code of
  a 70-repository corpus instead.
- RepoBench-R and the competitor arms run on seeded samples (the competitors on
  the first 20 of the agent round's 50-instance Lite sample, seed 20261003), and
  every table states its instance count.
- Times are wall-clock on one loaded laptop and are not published as results.
