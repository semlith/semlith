# semlith

[![ci](https://github.com/semlith/semlith/actions/workflows/ci.yml/badge.svg)](https://github.com/semlith/semlith/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A fast local vector store for AI agents.

Point it at your docs, PDFs, notes and code. semlith chunks them, embeds them,
and keeps a quantized vector index next to a SQLite database — all on your
machine, nothing sent anywhere. An agent then asks a question in plain English
and gets back the handful of excerpts that actually matter, with file paths and
line numbers, instead of reading whole files and burning tokens on the way.

Think of it as a semantic cache for everything your agent needs to know.

- **Local.** No API keys, no network at query time. The embedding model is
  downloaded once and cached.
- **Fast.** Vector search runs on [turbovec](https://github.com/RyanCodrai/turbovec)
  (Google Research's TurboQuant, 4 bits per coordinate, SIMD scan).
- **Incremental.** Re-running `index` only re-embeds files whose contents
  changed, and drops files that disappeared.
- **Agent-native.** Ships an MCP server, so any MCP-capable agent can call it
  as a tool.

## Install

Requires a 64-bit machine and a Rust toolchain (1.85+).

```sh
git clone https://github.com/semlith/semlith
cd semlith
cargo build --release
# binary at ./target/release/semlith
```

## Quick start

```sh
# Index a directory. Creates ./.semlith and downloads the embedding model
# (~130 MB) the first time.
semlith index ~/notes ~/papers ./src

# Ask it something.
semlith search "how does the retry backoff work"

# What's in there?
semlith stats
```

Output looks like this:

```
1. 0.812  src/client.rs:120-158
   /// Retries use full jitter: the delay is uniform in [0, base * 2^attempt],
   /// capped at MAX_BACKOFF. ...

2. 0.744  docs/reliability.md:44-71
   ...
```

The `path:start-end` locator is directly usable — hand it to an editor, or read
just those lines instead of the whole file.

## Commands

| Command | What it does |
|---|---|
| `semlith index [PATHS...]` | Index files and directories (defaults to `.`). Re-run to update. |
| `semlith search <QUERY>` | Search. `-k N` for result count, `--json` for machine output. |
| `semlith stats` | File count, chunk count, model, index size. |
| `semlith files` | List indexed files. |
| `semlith forget <PATH>` | Drop one file from the store. |
| `semlith mcp` | Run as an MCP server over stdio. |
| `semlith models` | List available embedding models. |

Global: `--store <DIR>` picks the store directory (default `.semlith`, or the
`SEMLITH_STORE` environment variable).

## Using it from an agent

`semlith mcp` speaks MCP over stdio and exposes two tools, `semlith_search` and
`semlith_stats`. For Claude Code:

```sh
claude mcp add semlith -- /path/to/semlith --store /path/to/.semlith mcp
```

Or in an MCP client config:

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/path/to/semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

The server loads the embedding model at startup, so the first tool call is as
fast as the rest.

## What gets indexed

Everything under the given paths, except:

- files ignored by `.gitignore` (and hidden files)
- binaries — detected by a NUL byte in the first 8 KiB
- files larger than 8 MiB
- anything under a `.semlith` directory

PDFs are extracted to text automatically. Everything else is read as UTF-8.
Files are split into line-aligned chunks of up to 800 characters with two lines
of overlap, so a chunk boundary rarely cuts a match in half.

## How it works

```
files ──chunk──> text ──embed──> vectors ──quantize──> index.tv   (turbovec)
                  │
                  └──────────────────────────────────> store.db   (SQLite)

query ──embed──> vector ──search index.tv──> chunk ids ──lookup store.db──> excerpts
```

Two files live in the store directory:

- **`index.tv`** — the turbovec index. Holds only quantized vectors keyed by
  chunk id. TurboQuant is data-oblivious, so there is no training step and no
  rebuild as the corpus grows: add vectors, they are searchable.
- **`store.db`** — SQLite. Holds the chunk text, its file, and its line span,
  plus the content hash that makes re-indexing incremental.

A search embeds the query, gets ids from the index, and resolves them with one
SQLite lookup each. The index only ever needs to hold vectors, so it stays
small enough to sit in memory even for large corpora.

Embeddings default to `BGESmallENV15` (384 dimensions), which runs on CPU via
ONNX Runtime. Pick another with `semlith index --model <NAME>`; see
`semlith models`. The model is fixed when the store is created, since vectors
from two models are not comparable — to switch, delete the store and re-index.

## Performance

Measured on an 8-core Apple Silicon laptop with 8 GB of RAM, indexing 79 Rust
source files (1.5 MB, 2375 chunks):

| | |
|---|---|
| Query, warm | **~6 ms** — embed the query, scan the index, read the rows |
| Query, cold CLI start | ~250 ms, almost all of it loading the ONNX model |
| Indexing | ~13 chunks/sec, ~1.7 GB peak RSS |
| Re-index, nothing changed | 17 ms — hashes match, the model is never loaded |

The numbers that matter for an agent are the first and the last. `semlith mcp`
loads the model once at startup, so every tool call costs the warm figure; and
keeping a store current is nearly free, because unchanged files are skipped
before anything is embedded.

Indexing is the slow half, and that cost is the embedding model, not the index
— a transformer on CPU is simply not fast. If you have a large corpus and can
trade some retrieval quality for throughput, `--model AllMiniLML6V2` is about
1.8x faster (6 transformer layers instead of 12). Quantized variants such as
`BGESmallENV15Q` were *not* faster in testing on ARM, though they do use less
memory.

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) — it
covers the checks CI runs, how the code is laid out, and what is deliberately
out of scope.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test                  # unit tests, offline
cargo test -- --ignored     # end-to-end round trip; downloads the model
```

For how the pieces fit together and why, see
[docs/architecture.md](docs/architecture.md).

Everyone participating is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md). Found a security problem? Please read
[SECURITY.md](SECURITY.md) rather than opening an issue.

## Known limits

- Search is dense-vector only. Exact identifier lookup (`grep`-style) is still
  better served by `grep`; hybrid keyword + vector search is not implemented.
- One process at a time per store. Concurrent `index` runs will fight over
  `index.tv`.
- First-time indexing of a large corpus takes a while — see
  [Performance](#performance). Subsequent runs only touch what changed.
- Peak memory during indexing is around 1.7 GB. On a memory-tight machine,
  that is the number to watch.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Note that semlith downloads embedding model weights at runtime; those are
covered by their own licenses. The default, BAAI/bge-small-en-v1.5, is MIT.
