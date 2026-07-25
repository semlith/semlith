# Architecture

How semlith is put together, and why. If you are here to change something, this
is the context that makes the code make sense.

## The shape of the problem

An agent that needs to know something about a large corpus has two bad options:
read everything (expensive, and most of it is irrelevant) or guess which file to
open (usually wrong). What it wants is the two or three paragraphs that actually
answer the question.

That is a retrieval problem, and the useful property is that the expensive part
— turning text into vectors — only has to happen once per chunk, at index time.
Querying afterwards is one embedding plus a scan.

So semlith optimizes for a very specific shape: **indexing can be slow, querying
must not be.**

## The two-file store

A store directory holds exactly two things:

```
.semlith/
├── index.tv     turbovec index — quantized vectors, keyed by chunk id
└── store.db     SQLite — chunk text, file paths, line spans, content hashes
```

The split is the central design decision. The vector index holds **only**
vectors and ids. It never holds text. This matters because the index is the
thing that gets scanned on every query, and its size determines whether the
working set fits in memory. At 4 bits per coordinate and 384 dimensions, a chunk
costs 192 bytes in the index — so a million chunks is about 190 MB, while the
text those chunks came from could be gigabytes sitting harmlessly in SQLite.

The two halves are joined by one integer. A chunk's SQLite rowid *is* its
turbovec external id. There is no mapping table, because there is nothing to
map.

## Indexing

```
walk paths ──▶ read bytes ──▶ hash ──▶ unchanged? ──▶ skip
                   │
                   ▼
              extract text  (PDF → pdf-extract, else UTF-8)
                   │
                   ▼
              chunk_text()  (line-aligned, ≤800 chars, 2 lines overlap)
                   │
                   ├──▶ INSERT INTO files/chunks  ──▶ chunk ids
                   │
                   ▼
              batch of 32 ──▶ embed ──▶ normalize ──▶ add_with_ids
                                                          │
                   ┌──────────────────────────────────────┘
                   ▼
              prune vanished files ──▶ write index.tv ──▶ commit hashes
```

A few things in that flow are load-bearing:

**Files are hashed before anything else happens.** BLAKE3 over the file bytes,
compared against the hash recorded last time. This is what makes re-indexing an
unchanged corpus take milliseconds instead of hours, and it is why the embedding
model is never even loaded on a no-op run.

**Hashes are committed last, after `index.tv` is on disk.** A file's row is
inserted with an empty hash while its vectors are still in flight. If the
process dies mid-run, those files still have an empty hash, do not match on the
next run, and get re-indexed. The alternative — recording the hash up front —
would leave chunks in SQLite that no vector points at, silently unsearchable
forever. Being slow to recover is fine; being quietly wrong is not.

**Changed files are evicted before they are re-added.** `delete_file` returns
the old chunk ids so they can be removed from the vector index in the same
breath as the SQL delete. SQLite reuses rowids after deletion, so skipping the
eviction would eventually collide an old vector with a new chunk's id.

**The index is written via a temp file and a rename**, so an interrupted save
cannot leave a truncated `index.tv` behind.

## Chunking

Chunks are line-aligned and capped at 800 characters, with the last two lines
repeated into the next chunk.

The cap is not arbitrary. Transformer cost grows faster than linearly in
sequence length, so halving chunk size more than halves the per-chunk embedding
cost — measured, going from 1200 to 800 characters improved throughput by 1.58x
per chunk and cut peak memory by 0.5 GB. Smaller chunks also retrieve more
precisely and cost an agent fewer tokens to read. There is a floor below which a
chunk stops carrying enough context to be meaningful; 800 characters is
comfortably above it.

Line alignment is what makes the `path:start-end` locator in the output useful.
A chunk that started mid-line could not name where it came from.

Lines longer than the whole budget — minified JavaScript, embedded base64 — are
hard-split on a character boundary rather than emitted oversized, and all pieces
share the one line number.

## Searching

```
query ──▶ prefix ──▶ embed ──▶ normalize ──▶ index.search(k)
                                                  │
                                          (scores, chunk ids)
                                                  │
                                                  ▼
                                        SELECT ... WHERE c.id = ?
                                                  │
                                                  ▼
                                    Hit { score, path, lines, text }
```

Embeddings are L2-normalized on both sides, which makes turbovec's inner product
equal to cosine similarity. Scores land in roughly `[-1, 1]`, higher is better,
and they are approximate because the stored vectors are quantized.

BGE English models were trained asymmetrically: passages are embedded raw, but
queries want an instruction prefix. `query_text` adds it. Omitting it measurably
costs recall, which is why it is not a detail worth simplifying away.

If a chunk id comes back that SQLite does not know about, the hit is skipped
rather than failing the query. That means the two halves have drifted, which
should not happen — but returning four good results beats returning an error.

## The MCP server

`semlith mcp` is newline-delimited JSON-RPC 2.0 over stdio, hand-rolled in one
file. A tools-only MCP server needs `initialize`, `tools/list`, `tools/call` and
`ping`; that is small enough that a dependency would cost more than it saves.

Two rules govern it:

- **stdout is protocol.** Nothing else may write there. This is why `Semlith`
  has a `quiet` flag — the model-download progress bar would otherwise corrupt
  the stream.
- **Requests without an id are notifications** and must not be answered.
  `notifications/initialized` arriving right after the handshake is the common
  case.

Tool failures are returned in-band as `isError` content rather than as
protocol-level errors, so the agent sees what went wrong and can react instead
of the call simply failing.

The server calls `warm()` at startup — loading the ONNX model and preparing the
index's lazy caches — so the first tool call is not several hundred milliseconds
slower than the rest.

## Why these dependencies

| Crate | Why |
|---|---|
| [turbovec](https://github.com/RyanCodrai/turbovec) | TurboQuant is data-oblivious: no training, no rebuilds as the corpus grows. Add vectors, they are searchable. |
| rusqlite (bundled) | Bundled SQLite means no system dependency and one file to back up. |
| fastembed | Runs sentence-transformer models on CPU via ONNX Runtime, with model download and tokenization handled. |
| ignore | The `ripgrep` walker. Gets `.gitignore` semantics right, which is harder than it looks. |
| pdf-extract | Pure Rust, no external binary. |
| blake3 | Fast enough that hashing every file on every run is free. |

## Things that were considered and left out

**Hybrid keyword + vector search.** Dense retrieval is weak at exact identifier
lookup — searching for `EMBED_BATCH` is a job for `grep`. SQLite's FTS5 would
slot in naturally. It was left out because the tool is a vector store first, and
adding a second retrieval path means deciding how to fuse two score
distributions, which is a real design problem rather than a small addition.

**A daemon or server mode.** Query latency is already ~6 ms warm; the only cost
worth amortizing is model load, which `semlith mcp` already does. A network port
would also undermine the "everything local" property that is the entire point.

**Storing embeddings in SQLite too.** Tempting for a single-file store, but then
every query either scans blobs out of SQLite or duplicates them in memory.
Keeping vectors in a purpose-built index is what makes the query fast.

**Per-file license headers.** Apache-2.0 recommends but does not require them,
and the Rust ecosystem convention is the `license` field in `Cargo.toml` plus a
`LICENSE` file. Both are present.
