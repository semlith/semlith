# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-07-30

One query across several stores, so an agent working across repositories asks
one question instead of one per repository.

### Added

- **`--store` is repeatable on the read commands.** `search`, `stats`, `files`
  and `mcp` cover every store named:

  ```sh
  semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith
  ```

  ```
  1. 0.033  [api] src/lock.rs:14-31
  2. 0.032  [cli] src/main.rs:96-104
  2 hits in 4.1ms across 2 stores: api 1, cli 1
  ```

  `-k` stays global — ten results over three stores is ten results. Filters
  apply to every store, and a filter matching files in only one of them returns
  that one's hits rather than reporting that nothing matched. Stores whose
  embedding models differ can be searched together: each embeds the query with
  its own model, and nothing downstream compares two models' numbers.

- **Every hit says which store it came from**, in the text output, in `--json`
  as a `store` field, and over MCP. A store is named after the directory holding
  it, so `../api/.semlith` is `api`; two stores that would collide get their
  paths instead.

- **`SEMLITH_STORE` accepts a list**, split the way `PATH` is, so an MCP server
  definition can name several stores without a wrapper script.

- **One MCP server, several stores.** `semlith_search` gained an optional
  `store` array that narrows a query, the tool description lists the stores that
  are open, an unknown name comes back as a tool error naming the real ones, and
  `semlith_stats` reports one line per store.

  Adding a store is cheap: measured on three 300-file stores sharing a model,
  one query embed per search rather than one per store, a median 3.6ms for one
  store against 4.1ms for three, and a server holding 128 MB on one store
  against 130 MB on three — one loaded model, not three.

### Changed

- **A read command refuses a store path that is not already a store**, instead
  of creating one. `search`, `stats`, `files` and `mcp` exit non-zero naming the
  path. Previously a mistyped `--store` became an empty store that answered
  every question with nothing — invisible in a multi-store query, where the
  other stores still return hits. `index` still creates the store it is given.
- **`index`, `watch` and `forget` take exactly one `--store`** and exit non-zero
  if given more. They write, and a store has one writer.

### Notes

No schema change and no index format change: a 0.4.0 store is searched by 0.5.0
with no re-index, and a store written by 0.5.0 is read by the 0.4.0 binary. A
single-store invocation prints and serializes exactly what 0.4.0 did — the store
label is absent when there is nothing to tell apart.

## [0.4.0] - 2026-07-29

Keeps a store current while you work: files are re-embedded as they are saved,
and an agent already connected over MCP sees the change without restarting.

### Added

- **`semlith watch [PATHS...]`.** Stays running and re-embeds files as they are
  saved, so `semlith index` stops being something you have to remember.

  ```sh
  semlith watch ~/notes ./src
  ```

  It begins with the same incremental pass `index` runs — so whatever changed
  while nothing was watching is caught up — and then waits on filesystem events
  rather than polling. Measured on a 1000-file corpus: no measurable CPU over a
  60-second idle window, and about a second from a save to that text being
  searchable.

  New files are indexed, deleted files lose their chunks and vectors, and a
  rename moves the file rather than duplicating it. An editor that saves by
  writing a temp file and renaming it over the original is treated as an edit,
  not as a deletion followed by a new file. Ignore rules come from the same walk
  `index` uses, so `.gitignore`, hidden files and the store's own directory are
  skipped by construction rather than by a second set of rules.

  Events are batched until things go quiet for `--debounce` milliseconds (500 by
  default), so one save — or a formatter rewriting a file three times — costs one
  re-embed and one index write.

- **A long-running reader now sees another process's writes.** The store counts
  index rewrites, and a search reloads the vector index when that count has
  moved. In practice: run `semlith watch` beside your agent's `semlith mcp`
  server, and the agent's answers track your working tree with nothing
  restarted. The check is one SQLite read per search.

- **Ctrl-C stops a watcher cleanly.** `SIGINT` and `SIGTERM` end it at a batch
  boundary, so `index.tv` is written whole or not at all and no temp index is
  left behind. A second signal kills it outright. Unix only; on Windows the
  default terminate-immediately behaviour applies.

### Changed

- `semlith watch` holds the store's write lock for as long as it runs. A store
  has one writer, so `semlith index` against a watched store exits non-zero and
  names the watcher's process. Searching is unaffected.
- An indexing pass that changes nothing no longer rewrites `index.tv`.

### Compatibility

No schema change and no index format change. A 0.3.0 store is watched without
re-indexing, and a store written by 0.4.0 opens, searches and indexes under
0.3.0 — both verified against the 0.3.0 release binary. The only addition to the
store is one row in the existing `meta` table.

## [0.3.0] - 2026-07-29

Narrows a search to part of a corpus, so one store per repository can answer a
question about one subsystem.

### Added

- **`--path`, `--ext` and `--lang` on `semlith search`.** Each is repeatable.
  Repeats union, kinds intersect: `--ext rs --ext toml` is "Rust or TOML",
  `--path 'src/**' --ext md` is "Markdown, under `src`".

  ```sh
  semlith search "how does retry backoff work" --path 'src/http/**'
  semlith search "how does retry backoff work" --lang rust
  ```

  The filter is applied *before* either half of the search picks its results.
  Asking for eight hits inside a subdirectory returns the eight best hits in
  that subdirectory, not whatever survives filtering the eight best hits in the
  repository — which for a small subdirectory is usually nothing. Concretely:
  the vector index is scanned under a turbovec allowlist and the FTS5 query
  carries the same path predicate, both derived from one id-selection query, so
  rank fusion never sees a chunk one half was forbidden to return.

  Path patterns are SQLite `GLOB`. A pattern that does not start with `/` is
  anchored as `*/<pattern>` against the stored absolute path, so `src/**`
  works from any working directory; an absolute pattern means exactly itself.
  `*` crosses `/`, so `src/*` already reaches the whole subtree. Matching
  ignores case, so `--ext md` finds `README.MD`.

  A filter that selects no indexed file reports that, rather than reporting
  that the corpus does not match the query.

- **The `semlith_search` MCP tool takes the same filters**, as optional `path`,
  `ext` and `lang` arrays, which is the point of the release: an agent working
  on one subsystem can scope its question to that subsystem.

- **`semlith languages`** prints the language names `--lang` accepts and the
  extensions each covers. An unrecognised name is an error naming this command,
  not a silent empty result.

### Notes

No store format change. `files.path` has been recorded since 0.1.0, so
filtering works on an existing store with no re-indexing, and a store written
by 0.3.0 is readable by 0.2.0.

## [0.2.0] - 2026-07-29

Removes three of the four limitations 0.1.0 shipped with, cuts peak indexing
memory to a third, and changes the default embedding model.

### Changed

- **New default embedding model: `granite-embedding-small-english-r2`, int8.**
  Replaces `BGESmallENV15` for newly created stores. On a 6260-chunk benchmark
  of mixed Rust, Markdown and TypeScript it scored 16.00 code MRR@10 against
  BGE-small's 14.84, in a 52 MB download instead of 133 MB, at the same query
  latency. It is 384-dimensional, so the index geometry is unchanged.

  **Existing stores are not affected and are not migrated.** A store keeps the
  model it was built with, because vectors from two models are not comparable.
  To move an existing store onto the new default, delete the store directory
  and re-index.

- **Search is now hybrid.** Every query consults SQLite FTS5 for literal terms
  alongside the vector index, and the two rankings are fused by reciprocal rank.
  There is no flag: dense-only search cannot reliably find an exact identifier,
  which is one of the most common things asked of a code corpus. Existing
  stores have their keyword index built on first open, which costs no
  re-embedding.

  The `score` on a hit is now a fusion score rather than a cosine similarity.
  It orders results within one query and means nothing across queries.

- **ONNX Runtime thread count is chosen rather than defaulted.** On Apple
  silicon it is the performance-core count; elsewhere the full core count.
  ONNX Runtime synchronises threads per operator, so one thread on an
  efficiency core paces the whole batch — measured 16.5 chunks/sec at four
  threads against 13.9 at eight on a 4P+4E M1. Override with
  `SEMLITH_EMBED_THREADS`.

- **Minimum supported Rust version is now 1.89**, for `std::fs::File::try_lock`.

### Added

- **One writer per store.** An index run holds an OS advisory lock on the store
  for its duration. A second run exits non-zero naming the process that holds
  it, instead of interleaving writes until the vector index and the database
  disagree. The kernel releases the lock if the process dies, so an interrupted
  run leaves nothing to clean up and no stale file to delete by hand.

- `semlith models` lists the new default alongside fastembed's built-in models.

### Fixed

- A release whose git tag disagrees with `Cargo.toml` now fails the release
  build instead of publishing binaries labelled with a version they were not
  built from.

### Performance

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM, over corpora of
mixed Rust, Markdown and TypeScript.

| store | warm query, p50 | indexing | peak RSS |
|---|---|---|---|
| 1.2k chunks | 2.7 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | 5.4 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | 22.7 ms | 23.5 chunks/sec | 595 MB |

- **Peak indexing memory is now roughly 600 MB and no longer grows with the
  corpus**, against ~1.7 GB in 0.1.0. Across a 1.2k, 9.9k and 105k chunk corpus
  it varies by 7 percent, and the largest corpus uses the least. Two causes:
  embedding batches were being flushed once per file rather than once per
  batch, so a single large file could hold thousands of chunks in memory; and
  the batch size itself was three times larger than it needed to be.
- **Indexing throughput is up from ~13 to ~23 chunks/sec**, from sizing the
  ONNX Runtime thread pool to the machine's performance cores.
- **Retrieval quality**, measured end to end through the binary over a
  6527-chunk corpus with 1809 self-labelled queries:

  | | code MRR@10 | docs MRR@10 |
  |---|---|---|
  | 0.1.0 (BGE-small, dense only) | 14.84 | 15.09 |
  | 0.2.0 (granite int8, dense only) | 16.00 | 14.08 |
  | **0.2.0 as shipped (granite int8 + FTS5)** | **17.90** | **15.62** |

  The model change alone would have regressed prose retrieval. Keyword fusion
  more than recovers it, which is why the two ship together.

### Library API

`semlith` is a published crate, so these are breaking changes to its Rust API.
The library API is unstable before 1.0 and changes with the minor version.

- `DEFAULT_MODEL: EmbeddingModel` is replaced by `default_model() -> Model`.
- `Semlith::open` takes `Option<Model>` rather than `Option<EmbeddingModel>`,
  and `Semlith::model` returns `&Model`. `Model` is an enum over fastembed's
  built-ins and the new default, which is not one of them.
- New modules: `embed` (model identity and loading) and `lock` (store locking).


## [0.1.0] - 2026-07-26

First release.

### Added

- **Local semantic search over files.** `semlith index` walks paths, extracts
  text, chunks it, embeds it locally, and stores the result. `semlith search`
  returns the best-matching excerpts with their file path and line range.
- **turbovec-backed vector index.** Vectors are quantized to 4 bits per
  coordinate and searched with SIMD. No training step, no rebuild as the corpus
  grows.
- **SQLite for everything else.** Chunk text, file paths, line spans and content
  hashes live in `store.db`; the vector index holds only vectors keyed by chunk
  id.
- **Incremental indexing.** Files are content-hashed with BLAKE3, so re-running
  `index` only re-embeds what changed and prunes what disappeared from disk. A
  file's hash is committed only after the vector index is durable, so an
  interrupted run re-indexes rather than leaving chunks that can never match.
- **MCP server.** `semlith mcp` speaks JSON-RPC over stdio and exposes
  `semlith_search` and `semlith_stats`, so any MCP-capable agent can query the
  store as a tool. The embedding model is loaded at startup so the first call is
  as fast as the rest.
- **PDF support.** PDFs are extracted to text automatically. Parser panics are
  caught so one malformed file cannot abort an indexing run.
- **Sensible skipping.** Honours `.gitignore` (including outside git repos),
  skips hidden files, binaries, and files over 8 MiB.
- **Commands** for inspecting and maintaining a store: `stats`, `files`,
  `forget`, `models`.
- **Model selection.** `--model` picks the embedding model when a store is
  created; the choice is then pinned, since vectors from two models are not
  comparable.

### Performance

Measured on an 8-core Apple Silicon laptop with 8 GB of RAM over 79 Rust source
files (1.5 MB, 2375 chunks):

- Warm query: ~6 ms
- Cold CLI start: ~250 ms, almost all of it loading the ONNX model
- Indexing: ~13 chunks/sec, ~1.7 GB peak RSS
- Re-index with nothing changed: 17 ms

[Unreleased]: https://github.com/semlith/semlith/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/semlith/semlith/releases/tag/v0.5.0
[0.4.0]: https://github.com/semlith/semlith/releases/tag/v0.4.0
[0.3.0]: https://github.com/semlith/semlith/releases/tag/v0.3.0
[0.2.0]: https://github.com/semlith/semlith/releases/tag/v0.2.0
[0.1.0]: https://github.com/semlith/semlith/releases/tag/v0.1.0
