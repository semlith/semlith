<div align="center">

<img src="https://raw.githubusercontent.com/semlith/semlith/main/assets/semlith-logo.png"
     alt="Semlith" width="128" height="128">

# semlith

**A fast local vector store for AI agents** — index files once, keep it current
as you save, and answer questions across all of it in milliseconds without
leaving the machine.

[![ci](https://github.com/semlith/semlith/actions/workflows/ci.yml/badge.svg)](https://github.com/semlith/semlith/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/semlith.svg?logo=rust)](https://crates.io/crates/semlith)
[![downloads](https://img.shields.io/crates/d/semlith.svg)](https://crates.io/crates/semlith)
[![docs.rs](https://img.shields.io/docsrs/semlith?logo=docsdotrs&label=docs.rs)](https://docs.rs/semlith)

[![msrv](https://img.shields.io/badge/rust-1.89%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg)](#install)
[![mcp](https://img.shields.io/badge/MCP-server-6E56CF.svg)](https://modelcontextprotocol.io)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

Point it at your notes, code, PDFs, Office documents and notebooks. semlith
reads each of them as the text a person would see, chunks it, embeds it, and
keeps a quantized vector index next to a SQLite database — all on your machine,
nothing sent anywhere. An agent then asks a question in plain English and gets
back the handful of excerpts that actually matter, with file paths and line
numbers, instead of reading whole files and burning tokens on the way.

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
| `semlith stats` | File count, chunk count, model, shard count and memory budget, index size. |
| `semlith files` | List indexed files. |
| `semlith forget <PATH>` | Drop one file from the store. |
| `semlith mcp` | Run as an MCP server over stdio. |
| `semlith models` | List available embedding models. |
| `semlith languages` | List the language names `--lang` accepts. |

Global: `--store <DIR>` picks the store directory (default `.semlith`, or the
`SEMLITH_STORE` environment variable). `search`, `stats`, `files` and `mcp` read,
so the flag is repeatable and they cover every store named; `index`, `watch` and
`forget` write, so they take exactly one.

## Indexing a large corpus

A first index is bound by how fast a transformer runs on your CPU, so a corpus
of a hundred thousand chunks is an hour rather than a moment. Two things make
that hour survivable.

It says where it is:

```
  + ~/notes/2019/migrations.md
    18420/54103 files, 41230 chunks, 26 chunks/s, ~23m left
```

And it keeps what it has done. Every thirty seconds the vectors embedded so far
are written to disk and the files they cover are recorded as indexed — in that
order, so a file is never marked done before its vectors exist. Close the
laptop, hit Ctrl-C, lose power: re-run the same command and it continues, saying
how many files it skipped.

```
indexed 12043 files (38221 chunks) in 1420.6s — 18420 already indexed, 0 skipped, 0 removed
```

Searching a large store holds a bounded amount of memory rather than all of its
vectors. `SEMLITH_INDEX_MEMORY` is that bound in megabytes, 512 by default; the
store keeps the shards it is using and puts down the coldest to stay inside it.
A store that fits searches at full speed; a store past its budget pays to read
shards back on each query and says so on stderr rather than looking mysteriously
slower. `semlith stats` shows both numbers before you spend an hour finding out.

Checkpointing, the memory budget and shards are all properties of the store
layout introduced in 0.7.0, so they apply to stores created by 0.7.0 or later.
A store you already have keeps working exactly as it did; see
[Compatibility](#compatibility).

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

## Searching several stores at once

Your work is not one repository. Name several stores and one query covers all of
them:

```sh
semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith
```

```
1. 0.033  [api] src/lock.rs:14-31
   ...
2. 0.032  [cli] src/main.rs:96-104
   ...
2 hits in 4.1ms across 2 stores: api 1, cli 1
```

- **Every hit says which store it came from** — in the text output, in `--json`
  as a `store` field, and over MCP. With one store there is nothing to tell
  apart, so nothing is labelled and the output is exactly what it always was.
- **A store is named after the directory holding it**, so `../api/.semlith` is
  `api`. Two stores that would end up with the same name get their paths instead.
- **`-k` is global.** Ten results over three stores is ten results, not thirty.
- **Filters apply everywhere.** `--ext rs` over two stores searches the Rust in
  both, and a filter that matches files in only one of them returns that one's
  hits rather than reporting that nothing matched.
- **Stores may disagree about the model.** Each one embeds the query with its
  own, which is the only way to query it. Merging happens on rank, not on
  distance, so nothing compares two models' numbers.

`SEMLITH_STORE` takes a list, split like `PATH`, which is what an MCP server
definition wants:

```sh
export SEMLITH_STORE=~/work/api/.semlith:~/work/cli/.semlith
semlith stats
```

Two limits worth knowing:

- **Writes stay single-store.** `index`, `watch` and `forget` take exactly one
  `--store`; a store has one writer, and four locks with four failure modes is
  not an improvement. Run a `watch` per store if you want several kept current —
  a watched store is searchable inside a multi-store query, freshness included.
- **A store path that is not already a store is an error.** Read commands refuse
  it instead of creating an empty one, because a mistyped store answers every
  question with nothing and the other stores hide it.

Adding a store is cheap. Measured on three 300-file stores that share a model,
M1: one query embed per search rather than one per store, a median 3.4ms for one
store rising to 4.0ms for three, and an MCP server that has answered a query
holding 137 MB on one store and 137 MB on three — one loaded model, not three.

## Using it from an agent

`semlith mcp` speaks MCP over stdio and exposes five tools:

| Tool | What it does |
| --- | --- |
| `semlith_search` | Ranked excerpts with file and line range, with the same `path`/`ext`/`lang`/`store` narrowing as the CLI. |
| `semlith_stats` | What each open store holds, and the names the other tools accept. |
| `semlith_files` | Which files are indexed — so "not indexed" and "not discussed" stop looking the same. |
| `semlith_index` | Index a path into an open store, so a corpus becomes searchable mid-conversation. |
| `semlith_forget` | Drop one file from a store. The file on disk is untouched. |

The two write tools take the store's lock for the call and give it back. A store
another process is writing — `semlith watch`, say — comes back as a tool error
naming the holder rather than a corrupted index. When more than one store is
open they need a `store` argument, because a store takes one writer and there is
no "the" store to guess at.

Indexing a large tree takes longer than a client will wait for one tool call, so
`semlith_index` works to a time budget, reports what it did not reach, and
continues where it left off when it is called again. `SEMLITH_MCP_INDEX_BUDGET`
sets that budget in seconds if your client's tool timeout is unusual.

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

One server can hold several stores, which is how an agent working across
repositories asks one question instead of one per repository:

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/path/to/semlith",
      "args": ["--store", "/work/api/.semlith", "--store", "/work/cli/.semlith", "mcp"]
    }
  }
}
```

Every excerpt then names its store, and the tool description lists the stores
that are open so the agent knows what it may narrow to. An optional `store`
array restricts one query — useful only when the agent already knows which
corpus holds the answer:

```json
{ "query": "how does retry backoff work", "store": ["api"] }
```

A name that is not open comes back as a tool error listing the ones that are,
rather than as an empty result an agent would read as "the corpus does not
discuss this". `semlith_stats` reports one line per store, which is where those
names come from.

### Which protocol revisions

semlith implements MCP `2026-07-28`, `2025-11-25`, `2025-06-18` and
`2024-11-05`, and every one of them has a session in `tests/mcp.rs` proving it.
Clients built on `2026-07-28` — the revision that removed the `initialize`
handshake — get `server/discover` and per-request versions; every client
shipping today gets the handshake it expects. A revision semlith does not
implement is answered with one it does, rather than echoed back.

`2025-03-26` is deliberately not advertised. It is the one revision that
required JSON-RPC batching, and a client pinned to it is answered with
`2025-11-25`.

### Setting it up in your client

Every snippet below runs `semlith --store /path/to/.semlith mcp`. Repeat
`--store` to open several stores, or set `SEMLITH_STORE` to a
path-separator-delimited list instead. `cargo install semlith` puts the binary
at `~/.cargo/bin/semlith`, which is on your `PATH` in a shell but often not in
an editor launched from a desktop icon — those entries use the absolute path.

**Claude Code** — `claude mcp add`, or a committed `.mcp.json` in the project
root. The `--` matters: without it Claude Code reads `--store` as one of its own
flags.

```sh
claude mcp add semlith -- semlith --store /path/to/.semlith mcp
```

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**Claude Desktop** — `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows. Settings →
Developer → Edit Config opens it.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**OpenAI Codex** — `~/.codex/config.toml`, shared by the CLI, the IDE extension
and the desktop app. TOML, and the table is `mcp_servers` with an underscore.

```toml
[mcp_servers.semlith]
command = "semlith"
args = ["--store", "/path/to/.semlith", "mcp"]
```

`codex mcp add semlith -- semlith --store /path/to/.semlith mcp` writes the same
table.

**GitHub Copilot in VS Code** — `.vscode/mcp.json` for a workspace, or the
profile copy that `MCP: Open User Configuration` opens. The root key is
`servers`, not `mcpServers`.

```json
{
  "servers": {
    "semlith": {
      "type": "stdio",
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**GitHub Copilot CLI** — `~/.copilot/mcp-config.json`, or `/mcp add` in a
session. Its name for a stdio server is `local`, not `stdio`.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "local",
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"],
      "tools": ["*"]
    }
  }
}
```

**Cursor** — `~/.cursor/mcp.json` everywhere, or `.cursor/mcp.json` in one repo.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "stdio",
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**Windsurf** — `~/.codeium/windsurf/mcp_config.json`.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**Zed** — the `zed: open settings file` command. Zed calls MCP servers context
servers, and keys them under `context_servers`.

```json
{
  "context_servers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**Gemini CLI** — `~/.gemini/settings.json`, or
`gemini mcp add semlith semlith --store /path/to/.semlith mcp`.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**JetBrains** — Junie reads `~/.junie/mcp/mcp.json`, or `.junie/mcp/mcp.json`
per project; AI Assistant takes the same JSON under Settings → Tools → AI
Assistant → Model Context Protocol.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"]
    }
  }
}
```

**Cline** — the MCP Servers panel, Configure. Cline's own documentation gives
two different paths for the file it writes (`~/.cline/mcp.json` and
`~/.cline/data/settings/cline_mcp_settings.json`), so let the panel open it
rather than guessing.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["--store", "/path/to/.semlith", "mcp"],
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

**Goose** — `goose configure` → Add Extension → Command-line Extension, or
`~/.config/goose/config.yaml`. Goose calls them extensions and spells the
command `cmd`.

```yaml
extensions:
  semlith:
    type: stdio
    name: semlith
    enabled: true
    cmd: semlith
    args: ["--store", "/path/to/.semlith", "mcp"]
    timeout: 300
```

## What gets indexed

Everything under the given paths, except:

- files ignored by `.gitignore` (and hidden files)
- binaries — detected by a NUL byte in the first 8 KiB
- files larger than 8 MiB
- an archive that decompresses to more than 32 MiB of text
- anything under a `.semlith` directory

A file that fails one of those caps, or that cannot be read — corrupt,
truncated, password-protected — is counted in the run's `skipped` total and the
run carries on. An unreadable document has never been able to fail an indexing
run, and still cannot.

What is left is read as the text a person opening the file would see. The
extension decides, and it decides before anything looks at the bytes — which is
what lets a `.docx` be read at all, since it is a ZIP archive and the binary
check above would reject every one of them. Where a format has divisions a line
number cannot express, a marker line names them, so an excerpt says which slide
or which cell it came from.

| Extension | What is taken from it | Markers |
|---|---|---|
| anything else | The file, as UTF-8 | — |
| `.pdf` | The extracted text | — |
| `.ipynb` | Every cell in notebook order, source and outputs. Stream output and a result's `text/plain` are kept, truncated at 2000 characters each; images, widgets and other MIME types are dropped. | `# Cell 3 (code)`, `# Cell 1 (markdown)`, `# Output:` |
| `.html`, `.htm` | The page's text. Tags are removed, `<script>` and `<style>` contents with them, and character entities are decoded. | — |
| `.docx` | Paragraphs in document order, one per line. The cells of a table row are tab-separated. | — |
| `.pptx` | Each slide's text, slides in numeric order. Speaker notes are not included. | `# Slide 11` |
| `.xlsx` | Each sheet in workbook order, a line per row, tab-separated cells. Shared and inline strings are resolved. | `## Sheet: Q3 Notes` |
| `.odt`, `.odp`, `.ods` | The same, from OpenDocument's `content.xml`. | `# Slide 2 (Intro)`, `## Sheet: Q3 Notes` |

Two details worth knowing:

- **HTML keeps its line numbers.** Every newline in the source survives,
  including the ones inside the tags that were removed, so a hit's
  `file:line` range still points at the line of the file on disk where that
  sentence lives. An entity semlith does not recognise is left as it was
  written, since `&thing;` is likelier to be text about an entity than one.
- **A spreadsheet is indexed as its cached values.** Formulas are not
  evaluated; what is searched is what the last program to save the file wrote
  into the cells.

The 32 MiB decompression cap is separate from the 8 MiB file cap because
compression means the two are different numbers: a few hundred kilobytes of
zeros expand to gigabytes, and without a bound on what comes out, the size of a
run's largest allocation would be chosen by whoever wrote the file.

Files are split into line-aligned chunks of up to 800 characters with two lines
of overlap, so a chunk boundary rarely cuts a match in half.

## How it works

```
files ──chunk──> text ──embed──> vectors ──quantize──> index/*.tvim  (turbovec)
                  │
                  └──────────────────────────────────> store.db      (SQLite)

query ──embed──> vector ──search shards──> chunk ids ──lookup store.db──> excerpts
```

Two things live in the store directory:

- **`index/`** — the turbovec index, as fixed-size shards of 65536 vectors,
  each named for the first chunk id it holds. Holds only quantized vectors keyed
  by chunk id. TurboQuant is data-oblivious, so there is no training step and no
  rebuild as the corpus grows: add vectors, they are searchable. Splitting the
  index is what lets a search hold a few shards instead of the whole corpus, a
  save rewrite one shard instead of everything, and a long index run checkpoint
  as it goes. Stores created before 0.7.0 have a single `index.tv` instead and
  keep it.
- **`store.db`** — SQLite. Holds the chunk text, its file, and its line span,
  plus the content hash that makes re-indexing incremental.

A search embeds the query, gets ids from the shards it needs, merges their
rankings, and resolves the result with one SQLite lookup each. Nothing is read
from disk until a query needs it, so opening a store — which `stats`, `files`
and an idle MCP server all do — costs no vectors at all.

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

### What a store costs to hold

Measured with an MCP server on one store, comparing 0.6.0 against 0.7.0 over the
same three corpora. *Open* is the process after a handshake and `tools/list`,
before any question has been asked; *searching* is the same process after
twenty queries.

| chunks | 0.6.0 open | 0.7.0 open | 0.6.0 searching | 0.7.0 searching | median query |
|---|---|---|---|---|---|
| 700 | 137 MB | 133 MB | 139 MB | 137 MB | 3.7 ms |
| 7 000 | 141 MB | 133 MB | 144 MB | 144 MB | 7.5 ms |
| 70 000 | 180 MB | **133 MB** | 185 MB | 179 MB | 52.6 ms |

A hundredfold more corpus costs an open 0.7.0 store **0.8 MB**; the same corpus
cost 0.6.0 **43 MB**, because it read every vector the moment the store was
opened. An agent's server that is sitting there waiting to be asked something
now holds a model and nothing else.

Searching still holds the vectors it searches — 70 000 chunks is 43 MB and fits
inside the 512 MB default with room to spare. Past that budget the store keeps
what it can and reads the rest back per query, which is what makes a corpus
larger than memory searchable at all, and it is not free: the same 70 000-chunk
store squeezed into an 8 MB budget answered in 364 ms instead of 53 ms, holding
110–126 MB across two hundred queries.

Changing one file rewrites the shards it touches rather than the index. On a
store of 14 shards, re-indexing one changed file wrote 176 KB of a 1436 KB
index. Two shards, not one: the shard losing the old vector and the newest shard
taking the new one — so the saving appears once a store is more than two shards,
around 131 000 chunks at the default shard size.

An index run killed eight seconds in, on a 6000-file corpus: 0.6.0 kept **0**
chunks, 0.7.0 kept **1952**.

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

## Compatibility

[`docs/compatibility.md`](docs/compatibility.md) says which parts of semlith are
a contract — the CLI commands and flags, the MCP tool names and schemas, the
protocol revisions, the store on disk, the Rust API — and which parts are free
to change under you, such as ranking scores and the text printed for a person to
read. It also says plainly what a 0.x version number does and does not promise.

Stores carry a `format_version` from 0.6.0 on. A store written before that is
read as format 1 and never rewritten, and a binary that meets a store from a
newer format refuses it naming both numbers rather than misreading it.

0.7.0 creates format 2 stores, whose vectors are shards under `index/`. **It
reads every older store as it finds it** — searched, indexed into, never
migrated, `format_version` untouched — so upgrading costs an existing store
nothing. Going the other way is the break: a 0.6.0 binary refuses a format 2
store, and a 0.5.0 binary, which predates the key, would read one as an empty
corpus. To move an existing store onto the new layout, delete it and index
again; there is no migration that would not re-embed the corpus anyway.

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
- A store larger than `SEMLITH_INDEX_MEMORY` reads shards back from disk on
  every query, so the memory bound is bought with latency. The bound is the
  point — a corpus that does not fit in memory is searchable at all — but if
  your store fits comfortably, raising the budget is free speed.
- Checkpointing, the memory budget and one-shard saves need the 0.7.0 store
  layout. A store created by an earlier version keeps its single index file and
  behaves exactly as it did, which also means an interrupted index run on one
  still loses the run.
- The default model is English-only. `semlith models` lists multilingual
  alternatives, which must be chosen when the store is created.
- Search filters are SQLite `GLOB` patterns, so `*` crosses `/` and there is no
  distinct `**`, no regex, and no way to express "not this path". `--lang` maps
  a fixed table of extensions and never reads file contents, so a Perl script
  named `build` is not Perl as far as semlith is concerned.
- Results are not reranked. A cross-encoder over the top results would improve
  ordering, at a cost per query that a local tool should not pay by default.
- Multi-store search is a merge, not a joint ranking. Each store ranks its own
  chunks and the merge compares fused rank scores across them, so a store with
  nothing to say still offers its best hit. Ties go to the closer vector, which
  is measured against the store's own model — across two models that comparison
  is approximate, and it only ever decides between hits the rank evidence has
  already called equal.
- Stores are named by path; there is no registry of named stores, and no
  discovery. A store is searched because it was named, and its label comes from
  the directory holding it.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Note that semlith downloads embedding model weights at runtime; those are
covered by their own licenses. The default,
ibm-granite/granite-embedding-small-english-r2, is Apache-2.0.
