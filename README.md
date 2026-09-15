<div align="center">

<img src="https://raw.githubusercontent.com/semlith/semlith/main/assets/semlith-logo.png"
     alt="Semlith" width="128" height="128">

# Semlith

**A fast local vector store for AI agents** — index files once, keep it current
as you save, and answer questions across all of it in milliseconds without
leaving the machine.

[![ci](https://github.com/semlith/semlith/actions/workflows/ci.yml/badge.svg)](https://github.com/semlith/semlith/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/semlith.svg?logo=rust)](https://crates.io/crates/semlith)
[![downloads](https://img.shields.io/crates/d/semlith.svg)](https://crates.io/crates/semlith)
[![docs.rs](https://img.shields.io/docsrs/semlith?logo=docsdotrs&label=docs.rs)](https://docs.rs/semlith)

[![msrv](https://img.shields.io/badge/rust-1.90%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg)](#install)
[![mcp](https://img.shields.io/badge/MCP-server-6E56CF.svg)](https://modelcontextprotocol.io)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

Point it at your notes, code, PDFs, Office documents and notebooks. Semlith
reads each of them as the text a person would see, chunks it, embeds it, and
keeps a quantized vector index next to a SQLite database — all on your machine,
nothing sent anywhere. Screenshots and diagrams go in too, matched by what they
depict. An agent then asks a question in plain English and gets back the handful
of excerpts that actually matter, with file paths and line numbers, instead of
reading whole files and burning tokens on the way.

Think of it as a semantic cache for everything your agent needs to know.

- **Local.** No API keys, no network at query time. The embedding model is
  downloaded once and cached.
- **Fast.** Vector search runs on [turbovec](https://github.com/RyanCodrai/turbovec)
  (Google Research's TurboQuant, 4 bits per coordinate, SIMD scan).
- **Hybrid.** Every query searches meaning *and* literal terms, so
  `retry backoff` and `EMBED_BATCH` both land on the right chunk.
- **Visual.** Images are embedded with CLIP alongside the text, so a sentence
  finds the diagram it describes — the one thing in a folder no grep reaches.
- **Incremental.** Re-running `index` only re-embeds files whose contents
  changed, and drops files that disappeared.
- **Agent-native.** Ships an MCP server, over stdio or over HTTP, so any
  MCP-capable agent can call it as a tool.
- **Visible.** `semlith start` keeps every store current and serves a portal on
  `127.0.0.1` — your corpus, your searches and your agents' config, in a page
  that is compiled into the binary and loads with the cable unplugged.

## Install

One command. No Rust toolchain, no package manager, nothing installed first.

<!-- install-oneliners:start -->
**macOS and Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/semlith/semlith/main/install.sh | sh
```

**Windows**

```powershell
irm https://raw.githubusercontent.com/semlith/semlith/main/install.ps1 | iex
```
<!-- install-oneliners:end -->

The Windows line runs in PowerShell, and Windows PowerShell 5.1 — the one that
ships with Windows — is enough. Git Bash is not required for the install or for
anything afterwards.

The script picks the release for your machine, checks the download against the
release's `SHA256SUMS`, unpacks it into `~/.semlith/bin`, and hands off to
`semlith setup`, which puts that directory on your `PATH`, pre-downloads the
embedding model so your first `index` is not a silent wait, and registers
Semlith with the agents you say you use. Open a new shell and `semlith
--version` works.

`semlith setup` is also the repair command: run it again any time to fix `PATH`
or add a client, and it reports every step that is already done rather than
doing it twice. `semlith setup --yes` takes the default at every prompt, so a
script or an agent can install Semlith unattended.

`SEMLITH_VERSION` pins a release by its tag; `SEMLITH_HOME` moves where it
lands.

Later, `semlith upgrade` fetches the newest release, verifies it and swaps the
binary in place; `semlith upgrade --check` only says whether one exists. Neither
happens on its own — Semlith has no startup check, no timer and no update
banner.

Requires a 64-bit machine. Prebuilt binaries cover Linux (x86_64, aarch64),
Apple silicon macOS and Windows x86_64. The Linux binary needs nothing but
glibc and libstdc++ — no OpenBLAS, no OpenSSL — but it needs a recent glibc:
the prebuilt ONNX Runtime it links against requires 2.39, which rules out
Debian 12, Ubuntu 22.04 LTS, RHEL 9 and Amazon Linux 2023. The floor is that
library's rather than this crate's, and
[#57](https://github.com/semlith/semlith/issues/57) tracks moving it. On any of
those, `cargo install semlith` builds against the glibc you have and works. Intel macOS is not
supported: ONNX Runtime no longer publishes x86_64 macOS builds, so the
embedding backend cannot link there.

### Other ways

Homebrew, winget and Scoop are coming. Until then:

**From crates.io**, if you would rather build it yourself:

```sh
cargo install semlith
```

**From a release archive**, if you would rather place the binary by hand:

```sh
# The newest release, resolved rather than written down: a version in an
# example is a version that goes stale between releases.
VERSION=$(curl -fsSL https://api.github.com/repos/semlith/semlith/releases/latest | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
TARGET=aarch64-apple-darwin        # or x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu
curl -LO "https://github.com/semlith/semlith/releases/download/$VERSION/semlith-$VERSION-$TARGET.tar.gz"
curl -LO "https://github.com/semlith/semlith/releases/download/$VERSION/SHA256SUMS"
shasum -a 256 -c --ignore-missing SHA256SUMS
tar xzf "semlith-$VERSION-$TARGET.tar.gz"
sudo install "semlith-$VERSION-$TARGET/semlith" /usr/local/bin/
semlith --version
```

On Windows, unpack the `.zip` and move `semlith.exe` somewhere on your `PATH`.

**From source**, with a Rust toolchain (1.90+):

```sh
cargo install --git https://github.com/semlith/semlith
```

## Quick start

```sh
# Index a directory. Creates a store under ~/.semlith and downloads the
# embedding model (~52 MB) the first time.
semlith index ~/notes ~/papers ./src

# Ask it something.
semlith search "how does the retry backoff work"

# What's in there?
semlith stats

# Or see it, and keep it current as you save:
semlith start
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
| `semlith index [PATHS...]` | Index files and directories (defaults to `.`). Re-run to update. `--include-secrets` indexes what the deny-list otherwise refuses. |
| `semlith watch [PATHS...]` | Stay running and re-embed files as they are saved. `--debounce MS` to tune. |
| `semlith search <QUERY>` | Search. `-k N` for result count, `--json` for machine output, `--path`/`--ext`/`--lang` to narrow it, `--prefer code\|docs\|any` to lift one side of the corpus. |
| `semlith read <TARGET>` | One span or one symbol and nothing around it: `src/store.rs:1041-1080`, `src/store.rs:12`, or a name. The second stage after a search. |
| `semlith pattern <QUERY>` | Run a tree-sitter structural pattern over the indexed files of one language. `--lang` is required. |
| `semlith stats` | File count, chunk count, image count, model, shard count and memory budget, index size. |
| `semlith files` | List indexed files. |
| `semlith add <URL>` | Fetch one https URL into the store and index it: a page, a PDF, a file on GitHub. One request, no crawling, no credentials. |
| `semlith forget <PATH>` | Drop one file from the store. |
| `semlith symbol <NAME>` | The definition, its callers and callees, and the ring two hops out, in one answer. From the parsed syntax tree rather than a grep for `fn name`. |
| `semlith neighbors <NAME>` | What calls it and what it calls, one hop each way. `--kind` to follow one edge kind. |
| `semlith path <FROM> <TO>` | The shortest chain of edges between two symbols, or nothing if they are unconnected. `--depth` to search further. |
| `semlith ledger` | Print what agents retrieved from this store, newest first. `--last N`, `--verify`. Needs no key. |
| `semlith start [PATHS...]` | Own every registered store, keep them current, serve the portal on `127.0.0.1:7365` and answer MCP at `/mcp`. `--port`, `--debounce`, `--airgap`, `--no-ledger`, `--no-mcp-http`. |
| `semlith key show` \| `rotate` | Print the agent key that opens the HTTP MCP endpoint, and the stanza around it, or mint a new one. `--now` on `rotate` drops the previous key immediately. |
| `semlith adopt <DIR>` | Move an existing store directory into the store home and register it. `--root` re-points one whose corpus moved. |
| `semlith trust <DIR>` | Say that a store outside the store home may be opened, once. Nothing is moved. `--list` prints what is trusted. |
| `semlith mcp` | Run as an MCP server over stdio. Forwards to a running `semlith start` when there is one. |
| `semlith models` | List available embedding models. |
| `semlith languages` | List the language names `--lang` accepts. |
| `semlith setup [--yes]` | Put `~/.semlith/bin` on `PATH`, pre-fetch the model, register agent clients. Idempotent, so it is also the repair command. `--airgap` skips the model. |
| `semlith upgrade` | Replace this binary with the newest release, checksum-verified. `--check` only says whether one exists (exit 10 when it does). `--version <TAG>` pins one. Never runs on its own. |

Global: `--store <DIR>` picks the store directory, and `SEMLITH_STORE` does the
same from the environment. `search`, `stats`, `files` and `mcp` read, so the
flag is repeatable and they cover every store named; `index`, `watch` and
`forget` write, so they take exactly one.

With no flag, Semlith resolves a store itself, in this order: an existing
`.semlith` beside the directory in question; a registered store whose root is
that directory or an ancestor of it; otherwise a new store under
`~/.semlith/stores/`, registered against that directory. `semlith mcp` and
`semlith start` are the exception — with no flag they open *every* registered
store, because a client stanza cannot know which directory the agent will be
started in.

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

Checkpointing, the memory budget and shards are properties of the store layout,
so a store written by an older release keeps its own and keeps working exactly
as it did. [Compatibility](#compatibility) says which layout is which.

A run started from the portal can be paused and stopped. Pausing holds it
between files, keeping the store lock, so resuming costs nothing. Stopping
undoes everything that run embedded — the store is left exactly as it was
before it started, and indexing the same folder again begins at zero.

### Forgetting files, and deleting a store

`semlith forget <path>` drops one file's chunks and vectors. The portal's Files
page does the same for a set: tick the rows and forget them in one call, which
reports how many files and how many chunks went and names anything in the
selection that was never indexed.

`semlith drop <store>` deletes a store outright — its vectors, chunks, graph
and ledger, and the registry entry naming it — and the Stores page has the same
action behind a second click that says what goes. Neither touches the files
that were indexed; they delete what semlith derived from them. When a daemon is
holding the store, the delete goes through it, so the writer stops and releases
its lock before anything is removed.

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

## The portal

`semlith start` is one process that owns every store you have indexed. It takes
each store's write lock, watches its roots and re-embeds files as they are
saved — exactly as `semlith watch` does — and serves a page you can open:

```sh
semlith start
```

```
http://127.0.0.1:7365/?token=6f1c…
semlith: listening on 127.0.0.1:7365
semlith: opened api at /Users/you/.semlith/stores/api — watching 1 root(s)
```

That one URL is printed once, on stdout. Everything else the daemon says goes
to stderr, and the token never appears there.

Nine pages, and each one is the same answer the terminal gives:

- **Stores** — what is indexed, with the watcher's live feed beside it.
- **Files** — every indexed file with the `path`/`ext`/`lang` filters the CLI
  has, the reader that parsed each one, and when it was last indexed. Sorted and
  paged by the server, so ordering a column orders the store rather than the
  page.
- **Index** — a folder picker that indexes into a store with progress streaming
  as it goes, and a URL field that fetches one page or paper into the store.
- **Search** — the same fused search the CLI and the MCP tools run, previewing an
  image hit inline.
- **Graph** — the symbols and edges as a drawing: pick one and see what calls it
  and what it calls.
- **Languages** — every name `--lang` accepts, with the extensions and filenames
  that make it up.
- **Agents** — who is connected, the tools they can call, the stanza for every
  documented client, and the switch that opens and closes the HTTP endpoint.
- **Ledger** — what agents retrieved, which client asked, and what reading those
  files whole would have cost instead.
- **Privacy** and **About** — the claims below, and how to check them yourself.

With no store yet it opens on a welcome screen instead.

**It is not on the network.** `127.0.0.1` is the only address it binds and there
is no flag to change that. Every page and every `/api/` route needs the per-run
token, held in a `SameSite=Strict; HttpOnly` cookie — without it, 401 and an
empty body. The one other credential is the agent key, and it opens `/mcp` and
nothing else: a key sitting in a client's configuration file cannot rotate a
token, adopt a store or start an upgrade. Either way the `Host` header must be
`localhost`, `127.0.0.1` or `::1`; anything else gets 400.
Every response carries a `Content-Security-Policy` allowing only `'self'`, and
no CORS header is sent anywhere. Every byte the page loads — the script, the
stylesheet, the IBM Plex faces — is compiled into the binary, so it opens with
the cable unplugged. The Privacy page shows all of it, with a packet-capture
recipe if you would rather check than be told.

`--airgap`, or `SEMLITH_AIRGAP=1`, is the check that makes that falsifiable: it
refuses to download model weights at all and exits naming the cache path, so a
machine that pre-seeded `SEMLITH_MODEL_CACHE` can prove nothing was fetched.

### One writer, and why that stopped being a problem

A store has one writer, which used to mean choosing: a watcher holding the lock
so your store stayed current, *or* an agent able to call `semlith_index`. The
second was refused for as long as the first ran.

The daemon ends that without weakening the rule. It *is* the writer, and
everything else is a client of it: when a daemon is running, `semlith mcp`
finds it and forwards each call over loopback, so an agent's `semlith_index`
and the watcher share one writer and never collide. Your client configuration
does not change — it is still `semlith mcp` — and when no daemon is running,
`semlith mcp` opens the stores itself exactly as it always did.

```sh
semlith start                 # in one terminal, or as a login item
                              # your agent keeps using `semlith mcp`
```

The port is 7365 and it does not move. If something is already on it the daemon
exits saying so rather than quietly picking another, because a URL that wanders
is not a bookmark. `--port` or `SEMLITH_PORT` changes it deliberately.

## Where stores live

A new store goes in `~/.semlith/stores/<name>`, and
`~/.semlith/registry.json` records which directories it covers:

```sh
cd ~/work/api && semlith index .     # ~/.semlith/stores/api
semlith index ~/work/cli             # ~/.semlith/stores/cli
semlith mcp                          # serves both, with no path anywhere
```

`SEMLITH_HOME` moves the home. `--name` names a store explicitly; two
directories called `api` get `api` and `api-2` rather than being merged.

With no `--store` flag, Semlith resolves a store in this order:

1. `--store` or `SEMLITH_STORE`, which always win.
2. A `.semlith` directory beside the corpus, if there is one. **A local store
   always wins**, which is what keeps a setup that predates the store home
   working untouched; the only difference is a line on stderr saying `adopt`
   would move it into the home.
3. A registered store whose root is this directory or an ancestor of it, so
   running from `src/` reaches the store that covers the repository.
4. Otherwise a new store in the home, registered against this directory.

`semlith mcp` and `semlith start` are the exception: with no flag they open
*every* registered store, because a client stanza cannot know which directory
your agent will be started in.

To move an existing store into the home:

```sh
semlith adopt ./.semlith
```

Nothing is re-embedded and nothing about the store changes — `semlith stats` is
identical before and after, apart from where it says the store is. If a corpus
moved rather than the store, `semlith adopt ~/.semlith/stores/api --root
~/work/api-renamed` re-points it.


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

## The code graph

A store knows the structure of the code in it, not only its text. Indexing
extracts symbols and the edges between them with tree-sitter, on the same
changed-file path that drives re-embedding. So a file saved under `semlith
start` updates its edges in the same pass that updates its vectors, and there is
no build step and no artifact that can quietly go stale.

From 0.17.0 that covers **every language `--lang` accepts** — all forty-six of
them, read from the same table the search filter reads, so the two cannot
disagree. Through 0.16.0 it was six, which meant `--lang kotlin` narrowed a
search perfectly well and `semlith symbol` then answered nothing about the same
corpus.

Fourteen of the forty-six are markup, data or configuration, and their structure
is their symbol set: a YAML, TOML or JSON key, a Markdown heading, a Terraform
block label, a Dockerfile stage, a Makefile target, a GraphQL type, a protobuf
message, a SQL table or view, a CSS selector, an HTML element with an id. What
they reference is files — an include, an import, a source, a `FROM`, a
stylesheet or script `src`, the table a view selects from — so a change to a
base image or a shared module has a blast radius you can ask about.

A store written by an earlier version keeps working and gains the new languages
the next time those files are indexed.

```console
$ semlith symbol acquire
acquire method  src/lock.rs:32-85

$ semlith neighbors acquire
callers (9)
  open_store via calls (extracted)  src/daemon.rs:645
  run via calls (extracted)  src/daemon.rs:805
  lock via defines (extracted)  src/lock.rs:1
  run via calls (inferred)  src/watch.rs:102
  ...
callees (8)
  read via calls (resolved)  src/daemon.rs:83
  write via calls (ambiguous) · 5 definitions
  open via calls (ambiguous) · 10 definitions
  daemon_holds via calls (resolved)  src/lock.rs:107
  ...

15 targets outside this store, not listed (--all)

$ semlith path search_in record_retrieval
search_in and record_retrieval are not connected within 6 hops by resolved
edges. Ambiguous names were not crossed; --all-edges walks them and labels
what it finds.
```

From 0.16.0 `semlith symbol` answers with all of that at once — the definition,
the resolved callers and callees, and the ring two hops out — because asking what
a symbol is used to cost three calls, two of which you had to make before you
knew whether the first had found the right symbol. Every edge that has one also
names the line the call was written on, which is not the line the calling
function starts on. And a chain may cross a re-export: `pub use x as y`,
`export { x as y }`, `from x import y as z` and Go's aliased import become
`aliases` edges, so a walk that would once have said "not connected" about code
joined by a rename now follows it and says it did.

### Reading one span, and asking for a shape

A search answers *where*. `semlith read` is the second half of that:

```console
$ semlith read src/store.rs:1041-1080     # exactly those lines
$ semlith read record_retrieval           # exactly that symbol
$ semlith read shared
2 definitions of this name:
  shared function  src/one.rs:4-4
  shared function  src/two.rs:1-1
```

A name with several definitions gives you the list rather than picking one.
Reads come out of the store's own chunks, never off disk, so `read` can only
return what semlith was allowed to index in the first place.

`semlith pattern` asks the question a regex cannot:

```console
$ semlith pattern --lang rust '(call_expression function: (identifier) @called)'
src/lib.rs:1461-1461 @called  normalize(&mut vector)
...
```

The grammar is the one the graph already uses, so a language `pattern` accepts is
a language the graph carries edges for. A pattern that does not compile gives you
tree-sitter's own error, not an empty list — "no matches" would read as "the code
does not contain this shape".

### How a query is read

semlith looks at the shape of what you typed before it ranks anything. One token
of identifier characters is read as an identifier and weights the keyword half of
the search twice; anything else is read as a question and leaves the two halves
level. Every answer says which it decided:

```console
$ semlith search record_retrieval
...
identifier-shaped · keyword weighted 2×

$ semlith search 'where does a retrieval get written down' --prefer code
...
question-shaped · vector and keyword equal · prefer code
```

`--prefer code` lifts implementation over the prose about it and `--prefer docs`
does the opposite. Both are a bias rather than a filter, so `--prefer code` over a
corpus of prose still answers with the prose.

**Reverse reachability is not currently part of the product.** `neighbors`
answers what calls a symbol one hop back; walking every caller of every caller
to a depth is not a question any command here answers. The
[changelog](CHANGELOG.md) says when that changed and what replaced it.

**Every edge says how well it is supported.** A call whose name — or, from
0.15.0, whose module or receiver — the file also names was resolved by the file
itself and is marked `extracted`. Everything else is ranked against the
definitions the store actually holds: the calling file's own definition first,
then one whose file the source pointed at, then one in a file the caller imports,
then the case where the corpus holds only one definition of the name. One
survivor is `resolved`. Several are `ambiguous`, and the row says how many rather
than printing one of them as if it were the call. A bare name match with nothing
to rank is `inferred`.

Trust `extracted` and `resolved` as answers, `inferred` as a hint, and
`ambiguous` as a question. Four functions called `get` in four modules is the
normal case in real code, and one row saying so is more use than four rows that
each look like a call site.

**A path walks definitions, not names.** Every hop of a chain has to leave from
the definition it arrived at. That sounds obvious and was not true before
0.15.0: a chain could arrive at one `search` and leave from another, so every hop
was true and the chain was false, in output identical to a right answer. By
default `semlith path` now refuses to cross a name it cannot pin down and says
so. `--all-edges` walks them anyway and shows its work — both ends of every hop
with file and line, a seam where the chain changed subject, a count of the hops
by confidence, and the sentence "A hypothesis, not a finding." `--strict` says
the default out loud, for a script that would rather not rely on it.

`semlith neighbors --all` expands a collapsed row into its definitions, and also
lists the targets the store holds no definition for — calls into a dependency
nobody indexed. Those were left out silently before; "no callees" and
"everything this calls is outside the index" are different facts.

**Search uses it.** The top vector and keyword hits are mapped to the symbols in
them, expanded one hop, and the chunks those neighbours live in join the ranking
as a third fused list. That costs no embedding and no model call, and it reaches
the case neither other list can: a concept spread over files that share no
vocabulary. Every hit says which lists found it — `v` vector, `f` full text, `g`
graph — so a result the graph alone reached reads as a neighbour of a match
rather than as a match.

Six languages carry edges. Everything else is searchable exactly as it was, with
no symbols; the portal's About page lists the six that carry edges.

## Finding an image

A store holds pictures as well as text. `.png`, `.jpg`, `.jpeg`, `.webp` and
`.gif` are embedded with CLIP ViT-B/32's vision encoder into a
second vector space inside the store, and a text query is embedded with the
matching CLIP text encoder — that pairing is the whole trick, and it is why
there is one fixed pair here rather than a choice of image model.

```console
$ semlith search "the architecture diagram with the queue in it"
1. 0.331  i   docs/design/pipeline.png:1280x720 px
```

An image hit carries its path and its pixel size where a text hit carries a line
range — in the terminal, in `--json` and over MCP — because there is no excerpt
to print under it. The Files page lists it with `image` as its reader, and
`semlith forget docs/design/pipeline.png` takes away its row and its vector the
same way it takes away a file's chunks.

**It is not OCR.** An image is matched by what it depicts, so a photograph of a
whiteboard finds "a whiteboard covered in boxes and arrows" and a screenshot
dense with text ranks poorly against the words in it. CLIP has no text
recognition to fall back on; that is a limit to know rather than one to work
around.

The two model files are fetched on the first image a store indexes, never at
start, and they go to the same model cache the text model uses — so a store
that never holds an image never downloads them, and `--airgap` refuses each by
name before a socket is opened. Nothing about the store format moves: the table and the
`images/` directory are additive, and a binary that predates them reads such a
store as the text corpus it already was.

## The retrieval ledger

Every retrieval goes into the store it came from: the query, the client that
asked, the session it belonged to, how many hits came back, the excerpt tokens
the agent actually read, and the whole-file tokens reading those files would have
cost instead. Search and the graph tools alike, from an agent over stdio, from an
agent over the daemon's `/mcp` endpoint, from the command line and from the
portal.

```console
$ semlith ledger --last 3
20:14:31 d20345  claude-code      8 hits      6 ms  where is the writer lock taken
```

Each row carries the hash of the row before it, so an edited or removed row is
detectable rather than merely unlikely — an audit record rather than a log file.
`semlith ledger --verify` re-walks that chain and exits non-zero if it is broken,
naming the first row that does not verify. `semlith stats` prints one line of the
total: tokens not read, over how many of how many retrievals, with the coverage
and whether the figure was measured or modelled. Both sides of that ratio are
counted with the store's own embedding tokenizer, so it is a count rather than a
rule of thumb.

**Recording is on, and it is local.** Before 0.15.0 it was off unless
`semlith start --ledger` asked for it, which meant almost nobody had a ledger and
the savings figure had no denominator. Three things make the new default all
right, and each of them is checkable rather than asserted:

- The rows never leave the store they were written into. Same claim as everything
  else here, same way to check it: a packet capture.
- The daemon says so every time it starts —
  `ledger: recording (local only; --no-ledger to stop)`, or `ledger: off for this
  session`.
- Erasing every row is one `DELETE` against a SQLite file you already own.

`semlith start --no-ledger` records nothing for that session; `SEMLITH_LEDGER=0`
records nothing on that machine. The `--ledger` flag is gone rather than kept as
a switch that does nothing, so a script still passing it fails loudly and is
corrected once. The command needs no licence key, now or ever.

## Using it from an agent

`semlith mcp` speaks MCP over stdio, and `semlith start` answers the same ten
tools over HTTP:

| Tool | What it does |
| --- | --- |
| `semlith_search` | Where the answer is: path, line span, enclosing symbol, how it was found and whether the file has changed since it was indexed, with the same `path`/`ext`/`lang`/`store` narrowing as the CLI. `format: "excerpt"` returns the text instead. An image hit carries its pixel size in place of the line range. |
| `semlith_stats` | What each open store holds, and the names the other tools accept. |
| `semlith_files` | Which files are indexed — so "not indexed" and "not discussed" stop looking the same. |
| `semlith_index` | Index a path into an open store, so a corpus becomes searchable mid-conversation. |
| `semlith_add` | Fetch one https URL into a store and index it, so a page or a paper joins the corpus mid-conversation. |
| `semlith_forget` | Drop one file from a store. The file on disk is untouched. |
| `semlith_symbol` | Where a symbol is defined, read off the parsed syntax tree rather than matched in a comment or a string. |
| `semlith_neighbors` | What calls a symbol and what it calls, one hop each way, each edge saying how well supported it is. `all: true` expands a collapsed ambiguous row and lists the targets this store holds no definition for. |
| `semlith_path` | The shortest chain of resolved edges between two symbols, or a refusal when it cannot get there without crossing a name it cannot pin down. `all_edges: true` crosses them and labels the answer a hypothesis. |
| `semlith_languages` | Every name `lang` accepts, and the extensions and filenames behind each. A fact about the build, asked once instead of carried in every tool schema. |

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
repositories asks one question instead of one per repository. That
is what a bare `semlith mcp` already does — it opens every store in the
registry — so indexing a second repository needs no edit to any client's
configuration:

```sh
semlith index ~/work/api
semlith index ~/work/cli
```

To pin a server to a chosen set instead, name them:

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

Semlith implements MCP `2026-07-28`, `2025-11-25`, `2025-06-18` and
`2024-11-05`, and every one of them has a session in `tests/mcp.rs` proving it.
Clients built on `2026-07-28` — the revision that removed the `initialize`
handshake — get `server/discover` and per-request versions; every client
shipping today gets the handshake it expects. A revision Semlith does not
implement is answered with one it does, rather than echoed back.

`2025-03-26` is deliberately not advertised. It is the one revision that
required JSON-RPC batching, and a client pinned to it is answered with
`2025-11-25`.

### Setting it up in your client

No snippet below names a store. Most of them hand the client the HTTP endpoint
`semlith start` serves; the rest run `semlith mcp`, which proxies to the same
daemon. Either way the server opens every store registered in
`~/.semlith/registry.json` — which is every store `semlith index` has made —
plus a `.semlith` beside the directory the agent was started in, if there is
one. Index another repository and the agent that is already configured can
search it, with no edit to any of these files.

Every stanza below names `${SEMLITH_AGENT_KEY}` rather than the key itself. The
client expands it from the environment at start, and `semlith setup` writes a
block in your shell startup file that exports it by reading
`~/.semlith/agent.key` — so a rotation needs no file here rewritten, and no
configuration file on the machine carries the credential. `semlith key show`
prints the key if you would rather paste the literal value; the next section
explains where it comes from and how to rotate it. The endpoint answers a
request without that header with 401 and nothing else, so a stanza that drops it
fails to connect rather than failing to find anything.

`--store` still works and still wins when it is given: repeat it to open a
chosen set of stores, or set `SEMLITH_STORE` to a path-separator-delimited
list. A store that sits beside its corpus keeps working where it is, and
`semlith trust ./.semlith` is enough to keep using it where it is — from 0.14.0
a store semlith did not create is not opened until you have said so once,
because a `.semlith` directory can arrive inside a repository you cloned.
`semlith adopt ./.semlith` moves it into the home instead, so these stanzas
reach it with no flags.

`cargo install semlith` puts the binary at `~/.cargo/bin/semlith`, which is on
your `PATH` in a shell but often not in an editor launched from a desktop icon
— the editor and desktop entries below use the absolute path.

#### Terminal

**Claude Code** — `claude mcp add`, or a committed `.mcp.json` in the project
root. `--transport http` points it at the running daemon; without it, the `--`
matters, because Claude Code reads anything starting with a dash as one of its
own flags.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
claude mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

Or as a subprocess, which needs no key:

```sh
claude mcp add semlith -- semlith mcp
```

**OpenAI Codex** — `~/.codex/config.toml`, shared by the CLI, the IDE extension
and the desktop app. TOML, and the table is `mcp_servers` with an underscore. A
table with a `url` in it is a streamable-HTTP server; the header goes in an
inline `http_headers` table. `codex mcp add` writes the same entry, but it has
no flag for an arbitrary header: it takes the key from an environment variable
instead, so `export SEMLITH_KEY="$SEMLITH_AGENT_KEY"` has to live in your shell profile
rather than in the one command —
`codex mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

```toml
[mcp_servers.semlith]
url = "http://127.0.0.1:7365/mcp"
http_headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

**OpenCode** — `opencode.json` in the project root, or the same file under
`~/.config/opencode/`. The root key is `mcp`, the transport is spelled
`remote`, and an entry is ignored until `enabled` is true.

```json
{
  "mcp": {
    "semlith": {
      "type": "remote",
      "url": "http://127.0.0.1:7365/mcp",
      "enabled": true,
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
opencode mcp add semlith --url http://127.0.0.1:7365/mcp --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

**IO CLI** — `io.local.toml` in the project root. The servers are an array of
tables rather than a map, so each one is its own `[[mcp]]` block and the name
is a field inside it.

```toml
[[mcp]]
id = "semlith"
url = "http://127.0.0.1:7365/mcp"
headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

```sh
io mcp add semlith --url http://127.0.0.1:7365/mcp --header 'Authorization=Bearer ${SEMLITH_AGENT_KEY}'
```

**GitHub Copilot CLI** — `~/.copilot/mcp-config.json`, or `/mcp add` in a
session. Its name for a subprocess is `local` rather than `stdio`, but a
remote server is the ordinary `http`.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "tools": ["*"]
    }
  }
}
```

```sh
copilot mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

**Gemini CLI** — `~/.gemini/settings.json`. The key for a streamable-HTTP
server is `httpUrl`, not `url`; a plain `url` is read as the older SSE
transport and the connection fails.

```json
{
  "mcpServers": {
    "semlith": {
      "httpUrl": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
gemini mcp add --transport http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}" semlith http://127.0.0.1:7365/mcp
```

**Qwen Code** — `~/.qwen/settings.json`, the same schema as Gemini CLI down to
`httpUrl` winning over `url` when both are present.

```json
{
  "mcpServers": {
    "semlith": {
      "httpUrl": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
qwen mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

**Amp** — `~/.config/amp/settings.json`, or the editor extension's own
`settings.json`. The root key is the dotted string `amp.mcpServers`, which
means the entry lives inside a wider settings file rather than in one of its
own.

```json
{
  "amp.mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
amp mcp add semlith -- semlith mcp
```

**Crush** — `crush.json` in the project root. The root key is `mcp`, not
`mcpServers`, and the TUI reads the file once at start, so relaunch it after
editing.

```json
{
  "$schema": "https://charm.land/crush.json",
  "mcp": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Droid** — `~/.factory/mcp.json` for every project, or `.factory/mcp.json` in
one repository. `droid mcp add semlith http://127.0.0.1:7365/mcp --type http
--header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"` writes the same object.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
droid mcp add semlith http://127.0.0.1:7365/mcp --type http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

**Goose** — `goose configure` → Add Extension → Remote Extension, or
`~/.config/goose/config.yaml`. Goose calls them extensions, spells the
transport `streamable_http` with an underscore, and takes the address as `uri`
rather than `url`.

```yaml
extensions:
  semlith:
    type: streamable_http
    name: semlith
    enabled: true
    uri: "http://127.0.0.1:7365/mcp"
    headers:
      Authorization: "Bearer ${SEMLITH_AGENT_KEY}"
    timeout: 300
```

**Amazon Q Developer CLI** — `~/.aws/amazonq/mcp.json` for every workspace, or
`.amazonq/mcp.json` in one. A remote entry takes only `type` and `url` and
authenticates over OAuth, with nowhere to put a header, so this is the stdio
form, which needs no key: `semlith mcp` proxies to the running daemon.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"]
    }
  }
}
```

```sh
q mcp add --name semlith --command semlith --args mcp
```

**OpenClaw** — `~/.openclaw/openclaw.json`. The servers are nested two deep
under `mcp` then `servers`, and the transport field is called `transport`, not
`type`.

```json
{
  "mcp": {
    "servers": {
      "semlith": {
        "transport": "streamable-http",
        "url": "http://127.0.0.1:7365/mcp",
        "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
      }
    }
  }
}
```

```sh
openclaw mcp add semlith --url http://127.0.0.1:7365/mcp --transport streamable-http --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

**DeepSeek** — `~/.deepseek/mcp.json`, read by DeepSeek-TUI, which has since
renamed itself Codewhale and now looks in `~/.codewhale/mcp.json` first and
falls back to the old path. Either file takes `servers` or `mcpServers` as the
root key, and needs no transport field: a `url` is enough. `codewhale mcp add`
has no header flag, so the key goes in the environment:
`export SEMLITH_KEY="$SEMLITH_AGENT_KEY"`, then
`codewhale mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Warp** — `~/.warp/.mcp.json`, or Settings → AI → MCP servers → + Add, which
writes it for you. An entry carries exactly one of `command` or `url` and Warp
rejects one holding both.

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "start_on_launch": true
    }
  }
}
```

#### Editors

**GitHub Copilot in VS Code** — `.vscode/mcp.json` for a workspace, or the
profile copy that `MCP: Open User Configuration` opens. The root key is
`servers`, not `mcpServers`.

```json
{
  "servers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
code --add-mcp '{"name":"semlith","type":"http","url":"http://127.0.0.1:7365/mcp","headers":{"Authorization":"Bearer ${SEMLITH_AGENT_KEY}"}}'
```

**Cursor** — `~/.cursor/mcp.json` everywhere, or `.cursor/mcp.json` in one
repo. Cursor infers the transport from the presence of `url` and has no `type`
field of its own; adding one copied from another client confuses it.

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Windsurf** — `~/.codeium/windsurf/mcp_config.json`. The address key is
`serverUrl`, not `url`, and Windsurf reloads MCP servers only on a full
restart, not on a window reload.

```json
{
  "mcpServers": {
    "semlith": {
      "serverUrl": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Zed** — the `zed: open settings file` command. Zed calls MCP servers context
servers and keys them under `context_servers`. Its remote support has moved
between versions and older builds start local processes only, so this is the
stdio form, which needs no key: `semlith mcp` proxies to the running daemon.

```json
{
  "context_servers": {
    "semlith": {
      "source": "custom",
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["mcp"]
    }
  }
}
```

**JetBrains** — Junie reads `~/.junie/mcp/mcp.json`, or `.junie/mcp/mcp.json`
per project; AI Assistant takes the same JSON under Settings → Tools → AI
Assistant → Model Context Protocol. The transport is spelled `streamable-http`
with a hyphen.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Cline** — the MCP Servers panel, Configure. Cline's own documentation gives
two different paths for the file it writes, so let the panel open it rather
than guessing. The transport is `streamableHttp` in camel case; anything else
falls back to SSE and the endpoint answers 405.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamableHttp",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

```sh
cline mcp add semlith --transport http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}" http://127.0.0.1:7365/mcp --yes
```

**Roo Code** — `.roo/mcp.json` in the project, or the global file the MCP
Servers panel opens. Roo spells the same transport `streamable-http`, with the
hyphen, which is the one thing that does not copy across from a Cline config.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "alwaysAllow": ["semlith_search"]
    }
  }
}
```

**Kilo Code** — `.kilocode/mcp.json` in the project, or the global file from
the MCP Servers panel. The `type` is required here: without it Kilo Code picks
SSE and the connection fails.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "alwaysAllow": ["semlith_search"],
      "disabled": false
    }
  }
}
```

```sh
kilo mcp add semlith --url http://127.0.0.1:7365/mcp --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

**Continue** — one block file per server at
`~/.continue/mcpServers/semlith.yaml`, or the same entry inlined in
`config.yaml`. The headers hang off `requestOptions`, not off the server
itself.

```yaml
name: Semlith
version: 0.0.1
schema: v1
mcpServers:
  - name: semlith
    type: streamable-http
    url: "http://127.0.0.1:7365/mcp"
    requestOptions:
      headers:
        Authorization: "Bearer ${SEMLITH_AGENT_KEY}"
```

**Kiro** — `.kiro/settings/mcp.json` in the workspace, or
`~/.kiro/settings/mcp.json` for every workspace. The workspace file wins where
both name the same server, so an old copy in the repository quietly overrides
the one you just edited.

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "disabled": false,
      "autoApprove": ["semlith_search"]
    }
  }
}
```

```sh
kiro-cli mcp add --name semlith --command "semlith" --args "mcp" --scope global
```

#### Desktop apps

**LM Studio** — `~/.lmstudio/mcp.json`, reached from the Program tab → Install
→ Edit `mcp.json`. LM Studio follows Cursor's notation, so there is no
transport field and a `url` is enough.

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Claude Desktop** — `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows. Settings →
Developer → Edit Config opens it. Desktop starts local processes only and has
nowhere to put a header, so this is the stdio form and needs no key: `semlith
mcp` proxies to the running daemon. Quit and reopen the app after editing.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["mcp"]
    }
  }
}
```

### Connecting over HTTP

`semlith start` also answers MCP at `http://127.0.0.1:7365/mcp`, so a client
that speaks the HTTP transport needs no subprocess and no path. The endpoint is
authenticated by the agent key in `~/.semlith/agent.key`, which is created on
first start and does not change when the daemon restarts, when Semlith is
upgraded, or when the portal's session token is rotated — so a stanza carrying
it is written once and keeps working. The key is 32 bytes from the OS random
source, written `sml_` and hex at mode 0600, and it is never reminted unless
you ask. `semlith key show` prints the live key and the stanza around it.

Rotating it carries it forward. `semlith key rotate`, and the portal's Rotate
key button, rewrite every client configuration file on this machine that
already held the old key — the documented path for each client above, plus the
project-scoped files beside wherever the daemon was started. A file is only
touched when the exact old key appears in it, nothing is ever created, and both
the command and the page list what they changed. A client configured somewhere
else still needs the new stanza pasted in.

```sh
claude mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

`semlith key rotate` mints a new one. The previous key keeps working until the
running daemon exits, so a session already open finishes rather than dying
mid-call; `--now` drops it immediately. Claude Code is re-registered through its
own CLI, because it is the one client Semlith has a supported way to write a
config for, and every other client that holds the old key is named so you know
what to paste the new stanza into.

The stdio stanzas above stay exactly as they are: `semlith mcp` forwards to a
running daemon and falls back to opening the stores itself when none is
running, which is what makes it work whether or not `semlith start` is up. The
HTTP endpoint is for clients that would rather hold a URL than spawn a process.
Close it with `semlith start --no-mcp-http`, or from the portal's Agents page
while the daemon runs — closing it drops the route, not the daemon.


## What gets indexed

Everything under the given paths, except:

- files ignored by `.gitignore` (and hidden files)
- binaries — detected by a NUL byte in the first 8 KiB, except the five image
  formats, which are read as pictures rather than as text
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
| `.epub` | Every chapter, in the order the book's spine gives — not the order the filenames sort in. | `# chapter-3.xhtml` |
| `.rtf` | The document's text. Font and colour tables, style sheets, embedded pictures and revision metadata are skipped whole; `\'hh` and `\uN` escapes are decoded. | — |
| `.eml` | `From`, `To`, `Cc`, `Date` and `Subject`, then the body: the `text/plain` part of a multipart, or its HTML part when there is no plain one. | `# Attachment: manifest.txt` |
| `.mbox` | Every message in the file, each read as an `.eml`. | `# Message 2: Inventaire de l'entrepôt` |
| `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif` | Nothing textual. The picture itself, embedded with CLIP — see [Finding an image](#finding-an-image). | — |

Two details worth knowing:

- **HTML keeps its line numbers.** Every newline in the source survives,
  including the ones inside the tags that were removed, so a hit's
  `file:line` range still points at the line of the file on disk where that
  sentence lives. An entity Semlith does not recognise is left as it was
  written, since `&thing;` is likelier to be text about an entity than one.
- **A spreadsheet is indexed as its cached values.** Formulas are not
  evaluated; what is searched is what the last program to save the file wrote
  into the cells.
- **A book is read in spine order.** Chapters are named `part0012.xhtml` as
  often as `chapter-three.xhtml`, so a reader that listed the archive would
  often open a book on its copyright page.
- **Mail keeps five headers and drops the rest.** A real message carries thirty,
  and twenty-five of them are routing, spam scoring and client fingerprints that
  are identical in every message a person owns. An attachment is named but never
  decoded.

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
  as it goes. An older store has a single `index.tv` instead and
  keep it.
- **`store.db`** — SQLite. Holds the chunk text, its file, and its line span,
  plus the content hash that makes re-indexing incremental.

A store that holds images grows a third: **`images/`**, the same shard layout at
CLIP's 512 dimensions. It is created on the first image and is absent otherwise.

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
upgrading Semlith never silently re-embeds a corpus.

Searches consult the vector index and SQLite's FTS5 keyword index together,
fusing the two rankings by position. Dense vectors alone are weak at exact
identifiers — every constant in a codebase embeds to roughly the same place —
and keywords alone cannot answer a question phrased as a sentence.

`--path`, `--ext` and `--lang` resolve to one set of chunk ids by a single
SQLite query against the stored file paths. That set becomes an allowlist the
vector index scans inside, and the same predicate goes into the FTS5 query, so
both halves rank within the subset and fusion never sees a chunk one half was
forbidden to return. Nothing is stored for it: the file path has been in the
database from the beginning, so filtering works on an existing store with no
re-indexing.

## Performance

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM, and reproducible:

```sh
cargo test --release --test measure -- --ignored --nocapture
```

Every figure in the first four tables was taken for this release. The
larger-corpus numbers below them are fixtures — building a
hundred-thousand-chunk store takes over an hour of embedding — and are re-taken
when the indexing or scan path changes rather than every release.

### Answering a question

Warm, server-side, as the daemon reports it. A query is embedded once per vector
space the store holds, and that embedding is most of the cost at these sizes.

| store | p50 | what is in it |
|---|---|---|
| text | **16.0 ms** | 2 220 chunks over 85 files |
| images | **9.1 ms** | 120 images, no text |
| a fleet of both kinds | **25.0 ms** | the text store above, beside one holding images |

The two halves add rather than interfere, because each store embeds the query
once for every vector space it holds. A store of source code never pays for the
image space at all: it has no images to compare against, so that half is skipped
before a model is loaded.

Over three larger corpora of mixed Rust, Markdown and TypeScript:

| store | warm query, p50 | p95 | indexing | peak RSS |
|---|---|---|---|---|
| 1.2k chunks | **2.7 ms** | 3.5 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | **5.4 ms** | 11.0 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | **22.7 ms** | 67.4 ms | 23.5 chunks/sec | 595 MB |

**Peak memory does not grow with the corpus** — 105k chunks is 85 times the work
of 1.2k for slightly less memory, so the number to plan for is roughly 600 MB
whatever you point it at. **Query latency does grow**, because the index scan is
linear: budget a few milliseconds for a repository and a few tens for a very
large corpus. This release did not touch either path; the recipe for rebuilding
the fixture is in [CONTRIBUTING.md](CONTRIBUTING.md).

### Keeping it current

| what | measured |
|---|---|
| edit on disk to searchable | **1.24 s**, watcher running |
| ten saves in six seconds | **1** index write of 638 KB |
| re-indexing 120 unchanged files | **0.01 s** — content hashes match, no model is loaded |
| idle daemon | **0.01 s** of CPU over 60 s, 10 MB resident |
| 100 edits | 37 MB resident at start, 59 MB after |

### Images

| what | measured |
|---|---|
| indexing 120 images | **10.0 s**, including loading the vision encoder |
| warm image query | **9.1 ms** p50 over 15 queries |
| the image index | 600 KB for 120 images, beside the text index |
| resident, after an image query | 191 MB |

CLIP is fetched only when a store first indexes an image: 335 MB for the vision
encoder and 244 MB for the text encoder, against 52 MB for the text model. A
corpus with no pictures in it never downloads either.

### What an agent pays

| what | measured |
|---|---|
| `tools/call` over HTTP | **23.4 ms** p50 over 20 calls |
| the same call through the stdio proxy | **20.9 ms** p50, same daemon, same query |
| `tools/list` | **3 940 bytes**, about 985 tokens, for ten tools with one store open — from 8 634 bytes for nine tools measured the same way in 0.14.0. `tests/retrieval.rs` asserts it stays under 1 000 tokens |
| an MCP server open on one store | 131 MB; on three stores, 132 MB |
| three same-model stores, one search | **1** query embed, +1.7 ms for the second store |

The endpoint costs about what the proxy costs, and the difference is the
client's connection setup rather than the route. Both talk to the same daemon
and run the same search.

### What sharding costs

A store split into 16 shards answers with the same top ten as the same corpus in
one shard **about 90%** of the time, measured over twelve questions about meaning
rather than about an identifier. The shards are what keep peak memory flat, and
that is the price.

The figure moves between 0.88 and 0.98 from run to run, and the reason is worth
knowing: ONNX Runtime reduces across its threads in whatever order they finish,
so the same question does not embed to exactly the same vector twice, and a
vector that lands nearer a tie changes which of two near-equal chunks comes
back. It is the same effect that makes `SEMLITH_EMBED_THREADS` worth pinning
when you want a reproducible index.

The binary is **45.6 MB**, of which 1.8 MB is the image support.

### What a store costs to hold

An MCP server that is sitting there waiting to be asked something holds a model
and nothing else: a hundredfold more corpus costs an open store **0.8 MB**,
because the vectors are read when a question is asked rather than when the store
is opened. Searching holds the vectors it searches — 70 000 chunks is 43 MB, and
fits inside the 512 MB default with room to spare. Past that budget the store
keeps what it can and reads the rest back per query, which is what makes a
corpus larger than memory searchable at all, and it is not free: the same
70 000-chunk store squeezed into an 8 MB budget answered in 364 ms instead of
53 ms.

Changing one file rewrites the shards it touches rather than the index. On a
store of 14 shards, re-indexing one changed file wrote 176 KB of a 1 436 KB
index — two shards, not one, because the shard losing the old vector and the
newest shard taking the new one both change. The saving appears once a store is
more than two shards, around 131 000 chunks at the default shard size.

An index run killed eight seconds into a 6 000-file corpus keeps what it had
already embedded: **1 952** chunks, not zero. Vectors are made durable before
the files they cover are marked indexed, so a kill is resumable and never leaves
a file recorded as indexed that cannot be answered for.

### Choosing a model

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
cores a thread on a slow core paces the whole batch; Semlith uses the
performance-core count on Apple silicon and the full count elsewhere. Override
with `SEMLITH_EMBED_THREADS` if your machine disagrees. It is also what makes
embedding reproducible: ONNX Runtime reduces across its threads in whatever
order they finish, so the same text embedded twice differs in the last bits, and
under int8 quantisation that is enough to swap two near-equal chunks.

## Compatibility

[`docs/compatibility.md`](docs/compatibility.md) says which parts of Semlith are
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

0.13.0 does not move the format again. The images table and the `images/`
directory are additive, and a binary that knows nothing about either ignores
both — so a store indexed with images is still a text store to 0.12.0, and the
images come back the moment a 0.13.0 binary opens it.

Neither does 0.15.0. It adds five nullable columns — a resolution hint on each
edge, and four on the ledger's rows — by an `ALTER TABLE` that runs on open, so
nothing is migrated: a 0.14.0 binary opens a 0.15.0 store and never asks for
them, and a 0.15.0 binary reads a 0.14.0 store with NULL hints exactly as 0.14.0
did, filling them in for a file on the next `index` pass that touches it.

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
- Checkpointing, the memory budget and one-shard saves need the sharded store
  layout. A store written before it keeps its single index file and behaves
  exactly as it did, which also means an interrupted index run on one still
  loses the run. [Compatibility](#compatibility) says which is which.
- The default model is English-only. `semlith models` lists multilingual
  alternatives, which must be chosen when the store is created.
- Image search is not OCR, and the CLIP pair behind it is fixed — there is no
  `--model` for the image half. A screenshot of a wall of text is matched on
  looking like a wall of text, not on the words in it, and a store that indexes
  its first image downloads two more model files to do so.
- Reverse reachability over the code graph is not part of the product.
  `neighbors` goes one hop back and no command walks further.
- Search filters are SQLite `GLOB` patterns, so `*` crosses `/` and there is no
  distinct `**`, no regex, and no way to express "not this path". `--lang` maps
  a fixed table of extensions and never reads file contents, so a Perl script
  named `build` is not Perl as far as Semlith is concerned.
- Results are not reranked. A cross-encoder over the top results would improve
  ordering, at a cost per query that a local tool should not pay by default.
- Multi-store search is a merge, not a joint ranking. Each store ranks its own
  chunks and the merge compares fused rank scores across them, so a store with
  nothing to say still offers its best hit. Ties go to the closer vector, which
  is measured against the store's own model — across two models that comparison
  is approximate, and it only ever decides between hits the rank evidence has
  already called equal.
- There is no discovery: a store is searched because the registry lists it or
  because `--store` named it, and nothing goes looking for stores on the
  filesystem. Adopting one is an action you point at a directory.
- The install scripts are the only packaged install. Homebrew, winget and Scoop
  are not there yet, so the one-liner and `cargo install` are the two ways in.
- Nothing is code-signed or notarized. A curl download carries no macOS
  quarantine attribute and the Windows script calls `Unblock-File`, which is
  enough for both to run — but neither binary is signed.
- There is no native ARM64 Windows build and no Intel macOS build. ARM64 Windows
  runs the x64 binary under emulation; Intel macOS has no option, because ONNX
  Runtime no longer publishes `osx-x86_64`.
- `semlith upgrade` only replaces a binary in `~/.semlith/bin`. A `cargo install`
  or a hand-placed copy is left alone, with the matching upgrade command
  printed instead — replacing a file this tool did not put there is not its
  business.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Note that Semlith downloads embedding model weights at runtime; those are
covered by their own licenses. The default,
ibm-granite/granite-embedding-small-english-r2, is Apache-2.0. The CLIP ViT-B/32
pair a store fetches once it holds an image carries its own licence too.
