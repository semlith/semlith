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
- **Hybrid.** Every query searches meaning *and* literal terms, so
  `retry backoff` and `EMBED_BATCH` both land on the right chunk.
- **Incremental.** Re-running `index` only re-embeds files whose contents
  changed, and drops files that disappeared.
- **Agent-native.** Ships an MCP server, so any MCP-capable agent can call it
  as a tool.

## Install

Requires a 64-bit machine. Prebuilt binaries cover Linux (x86_64, aarch64),
Apple silicon macOS and Windows x86_64. Intel macOS is not supported: ONNX
Runtime no longer publishes x86_64 macOS builds, so the embedding backend cannot
link there.

**On Linux, install OpenBLAS.** turbovec links against a system BLAS; macOS uses
Apple's Accelerate framework, which ships with the OS, and Windows falls back to
a pure-Rust implementation, so this step is Linux-only.

```sh
sudo apt-get install libopenblas-dev     # Debian/Ubuntu
sudo dnf install openblas-devel          # Fedora/RHEL
sudo pacman -S openblas                  # Arch
```

### From crates.io

```sh
cargo install semlith
```

### From a release

Grab the archive for your platform from the
[latest release](https://github.com/semlith/semlith/releases/latest) and put the
binary on your `PATH`:

```sh
VERSION=v0.1.0
TARGET=aarch64-apple-darwin        # or x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu
curl -LO "https://github.com/semlith/semlith/releases/download/$VERSION/semlith-$VERSION-$TARGET.tar.gz"
tar xzf "semlith-$VERSION-$TARGET.tar.gz"
sudo install "semlith-$VERSION-$TARGET/semlith" /usr/local/bin/
semlith --version
```

Each release ships a `SHA256SUMS` file; check your download against it before
running it. On Windows, unpack the `.zip` and move `semlith.exe` somewhere on
your `PATH`.

### From source

Needs a Rust toolchain (1.89+):

```sh
cargo install --git https://github.com/semlith/semlith
```

If the build fails with `unable to find library -lopenblas`, that is the
OpenBLAS step above you missed.

## Quick start

```sh
# Index a directory. Creates ./.semlith and downloads the embedding model
# (~52 MB) the first time.
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
| `semlith watch [PATHS...]` | Stay running and re-embed files as they are saved. `--debounce MS` to tune. |
| `semlith search <QUERY>` | Search. `-k N` for result count, `--json` for machine output, `--path`/`--ext`/`--lang` to narrow it. |
| `semlith stats` | File count, chunk count, model, index size. |
| `semlith files` | List indexed files. |
| `semlith forget <PATH>` | Drop one file from the store. |
| `semlith mcp` | Run as an MCP server over stdio. |
| `semlith models` | List available embedding models. |
| `semlith languages` | List the language names `--lang` accepts. |

Global: `--store <DIR>` picks the store directory (default `.semlith`, or the
`SEMLITH_STORE` environment variable).

## Keeping the store current

`semlith index` is a snapshot of the moment it ran. Leave `semlith watch`
running instead and a file is re-embedded when you save it:

```sh
semlith watch ~/notes ./src
```

```
watching notes, src — 412 files, 2183 chunks (0 indexed at startup, 412 unchanged)
  ~ src/client.rs
  1 re-embedded, 0 removed, 6 chunks in 0.3s
```

It starts with the same incremental pass `index` does, so anything that changed
while it was not running is caught up first, and then it waits on filesystem
events — no polling, no rescanning. Measured on a 1000-file corpus: no
measurable CPU over a 60-second idle window, and about a second from saving a
file to that file's new text being searchable.

New files are indexed, deleted files lose their chunks, and a rename moves the
file rather than duplicating it. An editor that saves by writing a temp file and
renaming it over the original is handled as an edit. `.gitignore`, hidden files
and the store directory itself are skipped, exactly as `index` skips them.

Saves are batched: events are collected until things go quiet for `--debounce`
milliseconds (500 by default), so one save — or a formatter rewriting a file
three times — costs one re-embed and one index write.

Two things worth knowing:

- **`watch` holds the store's write lock for as long as it runs.** A store has
  one writer. While it is running, `semlith index` against that store exits
  non-zero and names the watcher's process; searching is unaffected.
- **An MCP server already running picks the changes up.** It reloads the vector
  index when the watcher replaces it, so an agent that connected an hour ago
  searches what you saved a second ago without restarting anything.

What it does not cover: changes made while it was not running are caught by its
next startup pass, not retroactively; network filesystems do not deliver
reliable events and are not supported; and on Linux a very large tree can
exhaust the per-user inotify watch limit, which is reported on stderr rather
than leaving a watcher that is running but watching nothing.

## Searching part of a corpus

One store per repository, and then ask it about one subsystem:

```sh
semlith search "how does retry backoff work" --path 'src/http/**'
semlith search "how does retry backoff work" --ext rs --ext toml
semlith search "how does retry backoff work" --lang rust
```

Each flag is repeatable. **Repeats union, kinds intersect** — so
`--ext rs --ext toml` means "Rust or TOML", while `--path 'src/**' --ext md`
means "Markdown, under `src`".

The filter is applied before either half of the search picks its results, not
after. Ask for eight hits inside a subdirectory and you get the eight best hits
in that subdirectory, not whatever survives filtering the eight best hits in the
repository — which, for a subdirectory that is a small part of the corpus, is
usually nothing.

Three things about the globs are worth knowing:

- **A relative pattern matches anywhere in the tree.** Paths are stored
  absolute, so `--path 'src/**'` is matched as `*/src/**` and finds
  `/home/me/proj/src/lib.rs` from any working directory. Start a pattern with
  `/` to mean exactly that path and nothing else.
- **`*` crosses `/`.** This is SQLite's `GLOB`, which has no separate `**`, so
  `--path 'src/*'` already reaches the whole subtree. Writing `src/**` is
  allowed and means the same thing.
- **Matching ignores case**, so `--ext md` finds `README.MD`.

A filter that selects no indexed file says so, rather than reporting that
nothing in the corpus matched the query:

```
$ semlith search "retry backoff" --path 'srv/**'
no files match the filter (store has 6527 chunks)
```

`--lang` is a fixed table of extensions, not content sniffing. Run
`semlith languages` to see it; an unrecognised name is an error, not a silent
empty result.

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
fast as the rest. It also notices when the store has been rewritten underneath
it — run `semlith watch` alongside and the agent's answers track your working
tree, with no restart of the server or the agent.

`semlith_search` takes the same filters as the CLI, as optional `path`, `ext`
and `lang` arrays, so an agent working on one subsystem can ask about that
subsystem instead of the whole repository:

```json
{ "query": "how does retry backoff work", "path": ["src/http/**"], "lang": ["rust"] }
```

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

Embeddings default to `granite-embedding-small-english-r2`, quantized to int8
(384 dimensions, ~52 MB), which runs on CPU via ONNX Runtime. Pick another with
`semlith index --model <NAME>`; see `semlith models`. The model is fixed when
the store is created, since vectors from two models are not comparable — to
switch, delete the store and re-index.

A store built by an earlier version keeps the model it was built with, so
upgrading semlith never silently re-embeds a corpus.

Searches consult the vector index and SQLite's FTS5 keyword index together,
fusing the two rankings by position. Dense vectors alone are weak at exact
identifiers — every constant in a codebase embeds to roughly the same place —
and keywords alone cannot answer a question phrased as a sentence.

`--path`, `--ext` and `--lang` resolve to one set of chunk ids by a single
SQLite query against the stored file paths. That set becomes an allowlist the
vector index scans inside, and the same predicate goes into the FTS5 query, so
both halves rank within the subset and fusion never sees a chunk one half was
forbidden to return. Nothing is stored for it: the file path has been in the
database since 0.1.0, so filtering works on an existing store with no
re-indexing.

## Performance

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM, over three corpora
of mixed Rust, Markdown and TypeScript:

| store | warm query, p50 | p95 | indexing | peak RSS |
|---|---|---|---|---|
| 1.2k chunks | **2.7 ms** | 3.5 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | **5.4 ms** | 11.0 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | **22.7 ms** | 67.4 ms | 23.5 chunks/sec | 595 MB |

Re-indexing 8334 unchanged files takes 1.7 seconds, because content hashes
match and the model is never loaded. Cold CLI start on the 105k store is about
430 ms, most of it loading the model and the index.

Two things are worth reading off that table. **Peak memory does not grow with
the corpus** — 105k chunks is 85 times the work of 1.2k for slightly less
memory, so the number to plan for is roughly 600 MB whatever you point it at.
**Query latency does grow**, because the index scan is linear: budget a few
milliseconds for a repository and a few tens for a very large corpus.

The numbers that matter for an agent are the query row and the re-index figure.
`semlith mcp` loads the model once at startup, so every tool call costs the warm
figure; and keeping a store current is nearly free.

Indexing is the slow half, and that cost is the embedding model, not the index
— a transformer on CPU is simply not fast. If you have a large corpus and can
trade some retrieval quality for throughput, `--model AllMiniLML6V2` is about
1.8x faster (6 transformer layers instead of 12).

Quantization is worth testing rather than assuming. The int8 build of the
default model is both smaller *and* faster than its fp32 build on ARM, while
BGE's quantized variants measured no faster than fp32 on the same machine —
whether int8 wins depends on the model's graph, not on the architecture alone.

Thread count is chosen rather than left to ONNX Runtime. Its threads
synchronise at every operator, so on a CPU with performance and efficiency
cores a thread on a slow core paces the whole batch; semlith uses the
performance-core count on Apple silicon and the full count elsewhere. Override
with `SEMLITH_EMBED_THREADS` if your machine disagrees.

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

- One writer per store. A second `index` run against a store already being
  indexed exits with an error naming the process that holds it, rather than
  waiting. Searching during an index run is fine. `semlith watch` is a writer
  for as long as it runs, so it blocks `index` on that store the whole time.
- `watch` misses nothing while it is running, but it is not a journal: changes
  made while it was down are picked up by its next startup pass, not
  reconstructed. It needs local filesystem events, so network mounts are out.
  On Windows, Ctrl-C terminates it immediately rather than at a batch boundary,
  which can leave a file to be re-indexed on the next run.
- First-time indexing is bound by transformer speed on CPU, at roughly 23
  chunks/sec — a 100k-chunk corpus takes over an hour. Subsequent runs only
  touch what changed, and cost seconds.
- Query latency grows with corpus size, from under 3 ms at a thousand chunks to
  low tens of milliseconds at a hundred thousand. The index scan is linear.
- The default model is English-only. `semlith models` lists multilingual
  alternatives, which must be chosen when the store is created.
- Search filters are SQLite `GLOB` patterns, so `*` crosses `/` and there is no
  distinct `**`, no regex, and no way to express "not this path". `--lang` maps
  a fixed table of extensions and never reads file contents, so a Perl script
  named `build` is not Perl as far as semlith is concerned.
- Results are not reranked. A cross-encoder over the top results would improve
  ordering, at a cost per query that a local tool should not pay by default.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Note that semlith downloads embedding model weights at runtime; those are
covered by their own licenses. The default,
ibm-granite/granite-embedding-small-english-r2, is Apache-2.0.
