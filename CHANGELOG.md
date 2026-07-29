# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-29

Removes three of the four limitations 0.1.0 shipped with, and changes the
default embedding model.

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

[Unreleased]: https://github.com/semlith/semlith/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/semlith/semlith/releases/tag/v0.2.0
[0.1.0]: https://github.com/semlith/semlith/releases/tag/v0.1.0
