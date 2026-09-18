<div align="center">

<img src="https://raw.githubusercontent.com/semlith/semlith/main/assets/semlith-logo.png"
     alt="Semlith" width="128" height="128">

# Semlith

**A fast local vector store and code graph for AI agents** — index your files
once, keep it current as you save, and answer questions across all of it in
milliseconds without leaving the machine.

[![ci](https://github.com/semlith/semlith/actions/workflows/ci.yml/badge.svg)](https://github.com/semlith/semlith/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/semlith.svg?logo=rust)](https://crates.io/crates/semlith)
[![downloads](https://img.shields.io/crates/d/semlith.svg)](https://crates.io/crates/semlith)
[![docs.rs](https://img.shields.io/docsrs/semlith?logo=docsdotrs&label=docs.rs)](https://docs.rs/semlith)

[![msrv](https://img.shields.io/badge/rust-1.90%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![platform](https://img.shields.io/badge/platform-linux%20%7C%20macos%20%7C%20windows-lightgrey.svg)](#install)
[![mcp](https://img.shields.io/badge/MCP-server-6E56CF.svg)](https://modelcontextprotocol.io)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

## What this is

An agent that needs to know something about a large corpus has two bad options:
read everything, which is expensive and mostly irrelevant, or guess which file
to open, which is usually wrong. What it wants is the two or three paragraphs
that actually answer the question.

Semlith finds those paragraphs. Point it at your code, notes, PDFs, Office
documents and notebooks; it reads each as the text a person would see, splits it
into chunks, and builds two indexes over them.

### A vector index, in plain terms

A model reads each chunk and turns it into a list of numbers — a few hundred of
them — positioned so that chunks *about the same thing* end up near each other,
whatever words they used. "Retries use full jitter" and "how does the backoff
work" land close together without sharing a single term. Searching is then
arithmetic: the question becomes a list of numbers by the same model, and the
nearest chunks come back. The expensive half happens once, when a file is
indexed, so semlith is built for a specific shape: **indexing may be slow;
querying may not be.**

Vectors alone are weak at exact names — every constant in a codebase embeds to
roughly the same place — so every query also runs the literal terms through a
keyword index and fuses the two rankings. `EMBED_BATCH` and *how does retry
backoff work* both land on the right chunk.

### A code graph, in plain terms

The same indexing pass parses every source file and writes down two things: a
**node** for each definition it finds — a function, a class, a method, a heading
in a Markdown file, a key in a YAML one — and an **edge** for each relationship
between two of them. This file defines that name; this function calls that one;
this module imports that one; this re-export stands for that definition. It is
extracted on the same changed-file pass that re-embeds the file, so there is no
build step and no artifact that can go stale against your working tree.

### Why the two together

Grep finds a string. A hosted embedding service finds text that reads like your
question, and sends your code to somebody else's machine to do it. Neither
answers *what calls this*, *what would break if I changed it*, or *where is the
thing that does this, called something I would never have guessed*.

Semlith answers all three, locally. A search asks the vector index and the
keyword index, then expands the best hits one hop through the graph and folds
what it reaches into the same ranking — which catches the case neither list can
reach alone: a concept spread across files that share no vocabulary. Every hit
says which lists found it, so a result the graph alone reached reads as a
neighbour of a match rather than as a match. Images go in too, embedded with
CLIP beside the text. There are no API keys and no network at query time.

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

The Windows line runs in PowerShell, and Windows PowerShell 5.1 is enough. The
script picks the release for your machine, checks the download against the
release's `SHA256SUMS`, unpacks it into `~/.semlith/bin`, and hands off to
`semlith setup`, which puts that directory on your `PATH`, pre-downloads the
embedding model, registers semlith with the agents you say you use, and installs
the daemon as a login service. `setup` is also the repair command, and `--yes`
takes every default so a script can run it unattended. `SEMLITH_VERSION` pins a
release by its tag, `SEMLITH_HOME` moves where it lands, `SEMLITH_NO_SERVICE=1`
skips the login service, and `semlith upgrade` swaps the binary for the newest release —
never on its own, since semlith has no startup check, no timer and no update
banner. Or build it with `cargo install semlith`, or take an archive from
[the releases page](https://github.com/semlith/semlith/releases).

**What a machine needs.** 64-bit, and one of: Apple silicon macOS, Linux on
x86_64 or aarch64, or Windows on x86_64. The Linux archives ship Microsoft's
`libonnxruntime.so` beside the binary rather than linking it, so the pair needs
nothing but glibc and libstdc++ — no OpenBLAS, no OpenSSL. Its floor is
**GLIBC_2.34** and GLIBCXX_3.4.22, measured over both files on every tagged
build, which covers Debian 12, Ubuntu 22.04 LTS, RHEL 9 and Amazon Linux 2023.
Intel macOS is not supported: ONNX Runtime no longer publishes `osx-x86_64`, so
the embedding backend cannot link there.

## Quick start

```sh
# Creates a store under ~/.semlith and downloads the model (~52 MB) once.
semlith index ~/notes ~/papers ./src

semlith search "how does the retry backoff work"   # ask it something
semlith stats                                      # what's in there
semlith start                                      # see it, and keep it current
```

```
1. 0.812  src/client.rs:120-158
   /// Retries use full jitter: the delay is uniform in [0, base * 2^attempt],
   /// capped at MAX_BACKOFF. ...
```

The `path:start-end` locator is usable as it stands: hand it to an editor.

## Commands

| Command | What it does |
|---|---|
| `semlith index [PATHS...]` | Index files and directories (defaults to `.`). Re-run to update. `--each` gives every path its own store instead of one shared store; `--projects <FOLDER>` takes the paths from the git repositories directly under a folder, and implies `--each`. `--include-secrets` indexes what the deny-list otherwise refuses. |
| `semlith watch [PATHS...]` | Stay running and re-embed files as they are saved. `--debounce MS` to tune. |
| `semlith search <QUERY>` | Search. `-k N` for result count, `--json` for machine output, `--path`/`--ext`/`--lang` to narrow it, `--prefer code\|docs\|any` to lift one side of the corpus. |
| `semlith read <TARGET>` | One span or one symbol and nothing around it: `src/store.rs:1041-1080`, `src/store.rs:12`, or a name. The second stage after a search. |
| `semlith pattern <QUERY>` | Run a tree-sitter structural pattern over the indexed files of one language. `--lang` is required; `--path` narrows it and `--offset` continues a listing the cap cut short. |
| `semlith stats` | File count, chunk count, image count, model, shard count and memory budget, index size. |
| `semlith files` | List indexed files. |
| `semlith add <URL>` | Fetch one https URL into the store and index it: a page, a PDF, a file on GitHub. One request, no crawling, no credentials. |
| `semlith forget <PATH>` | Drop one file from the store. The file on disk is untouched. |
| `semlith scan [STORE]` | List every file the store holds that semlith would refuse today — a credential the name does not admit to, a rule that has widened. Exits non-zero while any remain; `--forget` evicts them. |
| `semlith drop <STORE>` | Delete a store outright — its vectors, chunks, graph and ledger, and the registry entry naming it. The indexed files are untouched. |
| `semlith symbol <NAME>` | The definition, its callers and callees, and the ring two hops out, in one answer. From the parsed syntax tree rather than a grep for `fn name`. |
| `semlith neighbors <NAME>` | What calls it and what it calls, one hop each way. `--kind` to follow one edge kind, `--all` to expand collapsed rows. |
| `semlith path <FROM> <TO>` | The shortest chain of edges between two symbols, or nothing if they are unconnected. `--depth` to search further. |
| `semlith ledger` | Print what agents retrieved from this store, newest first. `--last N`, `--verify`. Needs no key. |
| `semlith start [PATHS...]` | Own every registered store, keep them current, serve the portal on `127.0.0.1:7365` and answer MCP at `/mcp`. `--port`, `--debounce`, `--airgap`, `--no-ledger`, `--no-mcp-http`. |
| `semlith key show` \| `rotate` | Print the agent key that opens the HTTP MCP endpoint, and the stanza around it, or mint a new one. `--now` on `rotate` drops the previous key immediately. |
| `semlith adopt <DIR>` | Move an existing store directory into the store home and register it. `--root` re-points one whose corpus moved. |
| `semlith trust <DIR>` | Say that a store outside the store home may be opened, once. Nothing is moved. `--list` prints what is trusted. |
| `semlith mcp` | Run as an MCP server over stdio. Forwards to a running `semlith start` when there is one. |
| `semlith models` | List available embedding models. See [docs/models.md](docs/models.md). |
| `semlith languages` | List the language names `--lang` accepts. |
| `semlith setup [--yes] [--register-all]` | Put `~/.semlith/bin` on `PATH`, pre-fetch the model, and register semlith in every agent client on the machine that has a registration command — at the scope that means every project, launching `semlith mcp`, so no configuration file carries the key. Idempotent, so it is also the repair command. `--register-all` also writes the configuration file of the clients that have no command, listing every path first. `--airgap` skips the model. |
| `semlith doctor [--fix]` | Per client: installed, registered, at what scope, and what to run otherwise. Plus the Privacy rules that are readings of this machine. `--fix` applies the repairs that narrow access to a path semlith owns. |
| `semlith upgrade` | Replace this binary with the newest release, checksum-verified. `--check` only says whether one exists (exit 10 when it does). `--version <TAG>` pins one. Never runs on its own. |

`semlith add` fetches over https only, refuses redirects that leave https, caps
the body at 32 MiB, writes only inside the store's own `downloads/`, and refuses
everything under `--airgap` or to a private or loopback address, so a URL cannot
read something inside your network. `SEMLITH_ADD_ALLOW_PRIVATE=1` lifts that
last rule, for a wiki on a LAN you own.

## Where stores live

A new store goes in `~/.semlith/stores/<name>`, and `~/.semlith/registry.json`
records which directories it covers, so `semlith index ~/work/api` then
`semlith index ~/work/cli` leaves `semlith mcp` serving both with no path
written anywhere.

With no flag, semlith resolves a store in this order: `--store` or
`SEMLITH_STORE`, which always win; a `.semlith` beside the corpus, which always
beats the home, so a setup predating the store home keeps working untouched; a
registered store whose root is this directory or an ancestor of it; otherwise a
new store in the home. `semlith mcp` and `semlith start` are the exception —
with no flag they open *every* registered store, because a client stanza cannot
know which directory your agent will be started in. `search`, `stats`, `files`
and `mcp` read, so `--store` is repeatable; `index`, `watch` and `forget` write,
so they take exactly one. `semlith adopt ./.semlith` moves an existing store
into the home without re-embedding anything.

## Keeping it current

`semlith index` is a snapshot of the moment it ran. `semlith watch` re-embeds a
file when you save it: it starts with the same incremental pass `index` does, so
anything that changed while it was down is caught up first, then waits on
filesystem events rather than polling. Saves are batched until things go quiet
for `--debounce` milliseconds, so a formatter rewriting a file three times costs
one re-embed. It holds the store's write lock while it runs, so `semlith index`
against that store exits non-zero and names the holder; searching is unaffected,
and an MCP server already running picks the changes up without a restart.

## The portal

`semlith start` is one process that owns every store you have indexed: it takes
each store's write lock, watches its roots and re-embeds files as they are
saved, and serves a page you can open.

```
$ semlith start
http://127.0.0.1:7365/?token=6f1c…
semlith: listening on 127.0.0.1:7365
semlith: opened api at /Users/you/.semlith/stores/api — watching 1 root(s)
```

That URL is printed once, on stdout. Everything else goes to stderr, and the
token never appears there.
Ten pages — Stores, Files, Index, Search, Graph, Agents, Ledger, Privacy, Doctor
and About — each the same answer the terminal gives. The Graph page draws the
symbols and edges the store holds, with a filter chip per edge kind and a
confidence colour per edge. **[docs/portal.md](docs/portal.md) documents every
page**, what each control does, and what each column, badge and number means.

**It is not on the network.** `127.0.0.1` is the only address it binds and there
is no flag to change that. Every page and every `/api/` route needs the per-run
session token, sent in a `Semlith-Token` request header, and answers 401 with an
empty body without it. The agent key is the other credential and it opens `/mcp`
and nothing else, so a key sitting in a client's configuration file cannot
rotate a token, adopt a store or start an upgrade. Every response carries a
`Content-Security-Policy` allowing only `'self'`, no CORS header is sent, and
every byte the page loads is compiled into the binary. `--airgap` makes that
falsifiable: it refuses to download model weights at all and exits naming the
cache path. [docs/security.md](docs/security.md) is the full account, and the
Privacy page checks each claim on the running daemon rather than restating it.

The daemon also ends the one-writer trade-off without weakening the rule: it
*is* the writer, and while it runs `semlith mcp` finds it and forwards each call
over loopback, so an agent's `semlith_index` and the watcher never collide.

## Searching

```sh
semlith search "how does retry backoff work" --path 'src/http/**'
semlith search "how does retry backoff work" --ext rs --ext toml --lang rust
semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith
```

Each flag is repeatable. **Repeats union, kinds intersect** — `--ext rs --ext
toml` means "Rust or TOML", while `--path 'src/**' --ext md` means "Markdown,
under `src`". The filter is applied before either half of the search picks its
results, so asking for eight hits inside a subdirectory gets the eight best hits
*in that subdirectory* rather than whatever survives filtering the eight best
hits in the repository. Patterns are SQLite `GLOB`, so `*` crosses `/` and
matching ignores case, and a filter that selects no indexed file says so rather
than reporting that nothing in the corpus matched. Naming several stores
searches all of them at once: every hit says which store it came from, `-k` is
global rather than per store, and merging happens on rank rather than on
distance, so nothing compares two models' numbers.

semlith reads the shape of what you typed before it ranks anything. One token of
identifier characters weights the keyword half twice; anything else is read as a
question and leaves the two level, and every answer says which it decided.
`--prefer code` lifts implementation over the prose about it — a bias, not a
filter.

## The code graph

All **46 languages** `--lang` accepts carry graph edges, read from the same
table the search filter reads, so the two cannot disagree. Fourteen of them are
markup, data or configuration, where the structure *is* the symbol set — a YAML
key, a Markdown heading, a Dockerfile stage, a SQL table, a CSS selector — and
what those reference is files, so a change to a base image or a shared module
has a blast radius you can ask about.

```console
$ semlith neighbors acquire
callers (9)
  open_store via calls (extracted)  src/daemon.rs:645
  run via calls (inferred)  src/watch.rs:102
callees (8)
  read via calls (resolved)  src/daemon.rs:83
  write via calls (ambiguous) · 5 definitions
15 targets outside this store, not listed (--all)

$ semlith path search_in record_retrieval
search_in and record_retrieval are not connected within 6 hops by resolved
edges. Ambiguous names were not crossed; --all-edges walks them and labels it.
```

**Every edge says how well it is supported.** A call whose name, module or
receiver the file also names was settled by the file itself and is `extracted`.
Everything else is ranked against the definitions the store holds: one survivor
is `resolved`, several are `ambiguous` and the row says how many rather than
printing one of them as if it were the call, and a bare name match with nothing
to rank is `inferred`. Trust the first two as answers, `inferred` as a hint and
`ambiguous` as a question — four functions called `get` in four modules is the
normal case in real code, and one row saying so beats four rows that each look
like a call site.

**A path walks definitions, not names.** Every hop has to leave from the
definition it arrived at, so `semlith path` refuses by default to cross a name
it cannot pin down; `--all-edges` walks them anyway and labels the answer a
hypothesis rather than a finding. `semlith read` is the second half of a
search — one span or one symbol, answered out of the store's own chunks rather
than off disk, so it can only return what semlith was allowed to index — and
`semlith pattern` asks what a regex cannot, with the grammars the graph uses.

## Finding an image

`.png`, `.jpg`, `.jpeg`, `.webp` and `.gif` are embedded with CLIP ViT-B/32's
vision encoder into a second vector space inside the store, and a text query
goes through the matching text encoder — that pairing is the whole trick, and it
is why there is one fixed pair rather than a choice of image model.

```console
$ semlith search "the architecture diagram with the queue in it"
1. 0.331  i   docs/design/pipeline.png:1280x720 px
```

**It is not OCR.** An image is matched by what it depicts, so a photograph of a
whiteboard finds "a whiteboard covered in boxes and arrows" and a screenshot
dense with text ranks poorly against the words in it. The model files are
fetched on the first image a store indexes, so a corpus with no pictures never
downloads them.

## The retrieval ledger

Every retrieval goes into the store it came from — the query, the client that
asked, the session, how many hits came back, and what the agent read — whether
it came from an agent over stdio, from the daemon's `/mcp` endpoint, from the
command line or from the portal.

```console
$ semlith ledger --last 3
20:14:31 d20345  claude-code      8 hits      6 ms  where is the writer lock taken
```

Each row carries the hash of the row before it, so an edited or removed row is
detectable rather than merely unlikely — an audit record rather than a log file,
and `semlith ledger --verify` names the first row that does not verify.
Recording is on by default and local: the rows never leave the store, the daemon
says on every start that it is recording and names the flag that stops it, and
erasing every row is one `DELETE` against a SQLite file you already own.
`--no-ledger` stops a session and `SEMLITH_LEDGER=0` stops a machine. The
command needs no licence key, now or ever.

## Using it from an agent

`semlith mcp` speaks MCP over stdio, and `semlith start` answers the same twelve
tools over HTTP at `/mcp`:

| Tool | What it does |
| --- | --- |
| `semlith_search` | Where the answer is: path, line span, enclosing symbol, how it was found and whether the file has changed since it was indexed, with the same `path`/`ext`/`lang`/`store` narrowing as the CLI. `format: "excerpt"` returns the text instead. |
| `semlith_read` | One span or one symbol and nothing around it — the second stage after a search, so an agent locates first and reads only what it needs. |
| `semlith_pattern` | A tree-sitter structural pattern over the indexed files of one language, with the same `path` narrowing as the rest and an `offset` that continues a truncated listing. |
| `semlith_stats` | What each open store holds, and the names the other tools accept. |
| `semlith_files` | Which files are indexed — so "not indexed" and "not discussed" stop looking the same. |
| `semlith_index` | Index a path into an open store, so a corpus becomes searchable mid-conversation. |
| `semlith_add` | Fetch one https URL into a store and index it. |
| `semlith_forget` | Drop one file from a store. The file on disk is untouched. |
| `semlith_symbol` | Where a symbol is defined, read off the parsed syntax tree rather than matched in a comment or a string. |
| `semlith_neighbors` | What calls a symbol and what it calls, one hop each way, each edge saying how well supported it is. |
| `semlith_path` | The shortest chain of resolved edges between two symbols, or a refusal when it cannot get there without crossing a name it cannot pin down. |
| `semlith_languages` | Every name `lang` accepts, and the extensions and filenames behind each. |

The write tools take the store's lock for the call and give it back; a store
another process is writing comes back as a tool error naming the holder rather
than a corrupted index. Indexing a large tree takes longer than a client will
wait, so `semlith_index` works to a time budget and continues where it left off.
A bare `semlith mcp` opens every store in the registry, so an agent working
across repositories asks one question instead of one per repository, and
indexing a second repository needs no edit to any client's configuration.
semlith implements MCP `2026-07-28`, `2025-11-25`, `2025-06-18` and
`2024-11-05`, each with a session in `tests/mcp.rs` proving it.

**`semlith setup` registers semlith in every client that has a registration
command, at the scope that means every project**; `semlith doctor` says which it
could not and what to run for them. [docs/clients.md](docs/clients.md) holds the
stanzas for all 27 and the HTTP transport, and `tests/clients.rs` launches each.

**A server that is silently absent does not exist.** `setup` installs the daemon
as a login service — launchd agent, systemd user unit, logon task — so it answers
before any client asks; `semlith start --no-service` removes it. `semlith doctor`
asks whether each client can actually reach it: it names the first step that
failed and the command that shows it, repairs a registration that cannot launch,
and catches what nothing else does — a server registered at user scope and
switched off for one directory, which every check run from elsewhere calls
healthy. `--brief` is one line and an exit code for a shell prompt or a
session-start hook, and [docs/clients.md](docs/clients.md) has that snippet.

## What gets indexed

Everything under the given paths except files ignored by `.gitignore`, hidden
files, binaries (a NUL byte in the first 8 KiB, the five image formats aside),
files over 8 MiB, an archive that decompresses to more than 32 MiB of text, and
anything under a `.semlith` directory. A file that fails a cap or cannot be read
is counted in the run's `skipped` total and the run carries on. What is left is
read as the text a person opening the file would see, decided by the extension
before anything looks at the bytes — which is what lets a `.docx` be read at
all, since it is a ZIP archive the binary check would reject.
Thirteen formats have a reader of their own, and where one has divisions a line
number cannot express, a marker line names the slide, sheet or cell. Everything
else is read as UTF-8, in line-aligned chunks of up to 800 characters with two
lines of overlap.

## How it works

```
files ──chunk──> text ──embed──> vectors ──quantize──> index/*.tvim  (turbovec)
                  │
                  └──────────────────────────────────> store.db      (SQLite)

query ──embed──> vector ──search shards──> chunk ids ──lookup store.db──> excerpts
```

A store holds `index/` — the [turbovec](https://github.com/RyanCodrai/turbovec)
index, shards of 65536 quantized vectors keyed by chunk id — and `store.db`, the
SQLite database holding chunk text, paths, line spans, symbols, edges, the
ledger and the content hashes that make re-indexing incremental. Sharding lets a
search hold part of the corpus, a save rewrite one shard, and a long run
checkpoint as it goes.
Embeddings default to `granite-embedding-small-english-r2` at int8, 384
dimensions, ~52 MB, on CPU via ONNX Runtime; the model is fixed when the store
is created, because vectors from two models are not comparable.
[docs/architecture.md](docs/architecture.md) is the full account.

## The numbers

Two kinds, kept apart because they go stale differently. **Coverage** is read
out of the code and asserted by `tests/readme.rs`, which fails the build if one
of these drifts from its source:

| | |
|---|---|
| languages searchable and graphed | **46** |
| edge kinds | **6** |
| document formats with a reader | **13** |
| image types | **5** |
| MCP tools | **12** |
| CLI commands | **25** |
| agent clients, each launched and answered in `tests/clients.rs` | **27** |
| prebuilt targets | **4** |

**Measured**, on a 4P+4E Apple Silicon laptop, with what reproduces each one:

| | | |
|---|---|---|
| peak RSS, at 1 229 / 9 893 / 104 816 chunks | **600 / 637 / 595 MB** | `cargo test --release --test measure -- --ignored --nocapture` |
| edit on disk to searchable | **under 5 s** | the same |
| idle watcher CPU, over 60 s | **under 1.0 s** | the same |
| one search across three stores | **1 query embed**, ~39 MB per extra store | the same |
| one changed file | **1 shard rewritten** | `cargo test --release --test shards -- --ignored --nocapture` |
| `tools/list` | **3 995 bytes**, ~999 tokens, twelve tools | `cargo test --release --test retrieval -- --ignored` |
| retrieval, on 30 sealed questions of 107 | **hit@1 56 %, hit@3 66 %, hit@8 73 %**, wrong-yes **0** | the same |
| the same binary, on the 77 it was tuned against | hit@1 68 %, hit@3 79 %, hit@8 87 % | the same |
| call-edge resolution | **66 %** settled, against a 50 % gate | the same |
| the macOS arm64 binary | **116 679 872 bytes** (111.3 MiB) | `ls -l target/release/semlith` |
| the Linux glibc floor | **GLIBC_2.34** | `objdump -T semlith runtime/libonnxruntime.so` |

Peak memory does not grow with the corpus — 105k chunks is 85 times the work of
1.2k for slightly less memory — so plan for roughly 600 MB whatever you point it
at. Query latency does grow: the index scan is linear.
[docs/performance.md](docs/performance.md) has the full tables.

## Known limits

- One writer per store. A second `index` run against a store already being
  indexed exits with an error naming the process that holds it.
- First-time indexing is bound by transformer speed on CPU, at roughly 23
  chunks/sec. Later runs touch only what changed.
- A store larger than `SEMLITH_INDEX_MEMORY` reads shards back from disk on
  every query. The bound is the point — a corpus larger than memory is
  searchable at all — but if your store fits, raising the budget is free speed.
- The default model is English-only and is fixed when a store is created. Image
  search is not OCR, and the CLIP pair behind it is fixed.
- Search filters are SQLite `GLOB`: no regex, and no way to express "not this
  path". `--lang` maps a fixed table of extensions and never reads contents.
- Results are not reranked by a cross-encoder, multi-store search is a merge
  rather than a joint ranking, and reverse reachability is not part of it.
- Nothing goes looking for stores on the filesystem, nothing is code-signed,
  there is no ARM64 Windows or Intel macOS build, and `semlith upgrade` only
  replaces a binary in `~/.semlith/bin`.

## Documentation

[docs/portal.md](docs/portal.md) covers every page of the portal, every control,
and the concepts behind the graph and the search list.
[docs/clients.md](docs/clients.md) holds the configuration stanzas for 27 agent
clients and the HTTP transport.
[docs/architecture.md](docs/architecture.md) is how the pieces fit together and
why, [docs/performance.md](docs/performance.md) every measured number with its
command and its date, [docs/models.md](docs/models.md) the embedding models and
how to choose one, and [docs/security.md](docs/security.md) the threat model and
what is on the wire. [docs/compatibility.md](docs/compatibility.md) says what is
a contract and what is free to change under you, including what a 0.x version
number does and does not promise; [CHANGELOG.md](CHANGELOG.md) says what changed.

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers the checks
CI runs, how the code is laid out, and what is deliberately out of scope;
[AGENTS.md](AGENTS.md) is the same ground for a coding agent. Everyone
participating follows the [Code of Conduct](CODE_OF_CONDUCT.md). Found a
security problem? Please read [SECURITY.md](SECURITY.md) rather than open an
issue.

## Prior art

Semlith's vector index is [turbovec](https://github.com/RyanCodrai/turbovec), by
Ryan Codrai, under the MIT licence. It implements TurboQuant, from ["TurboQuant:
Online Vector Quantization with Near-optimal Distortion
Rate"](https://arxiv.org/abs/2504.19874) by Amir Zandieh, Majid Daliri, Majid
Hadian and Vahab Mirrokni.

Quantizing a vector means keeping it in far fewer bits than it arrived in, which
is what lets a million chunks be searched from memory. Most quantizers learn how
from the data, so they need a representative sample of the corpus before they
can compress any of it. TurboQuant is data-oblivious: it rotates each vector
randomly and quantizes each coordinate on its own, with no sample and no
training pass.

That is what an index refreshed on every file save needs. Nothing is gathered
before the first file is indexed, nothing is rebuilt as the corpus grows, and a
vector added is a vector searchable.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

Note that semlith downloads embedding model weights at runtime; those are
covered by their own licenses. The default,
ibm-granite/granite-embedding-small-english-r2, is Apache-2.0, and the CLIP
ViT-B/32 pair a store fetches once it holds an image carries its own too.
