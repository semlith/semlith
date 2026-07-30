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
              extract text  (by extension: PDF, documents, else UTF-8)
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

## Extraction

Everything a file has to survive before it can be chunked happens in one
function, `chunk::extract`, and it dispatches on the extension before it looks
at a byte. That ordering is load-bearing twice over. A `.docx`, `.pptx`,
`.xlsx` or any OpenDocument file is a ZIP archive, so the NUL-byte check that
rejects binaries would reject every document if it ran first; and a corpus of
ordinary source, which has none of these extensions, pays one string comparison
per file and never enters a parser.

The readers live in `src/formats.rs`, private because what semlith extracts
from a document is documented behaviour rather than API. Six of the nine
formats are ZIP archives of XML, so they share one bounded archive reader and
one tag scanner rather than carrying six parsers; a notebook is JSON, which
serde_json already handles; HTML is a character scan that removes tags while
keeping every newline the source had, which is what lets a hit into an HTML
page still name the line of the file on disk.

Two rules hold across all of them. A file that cannot be read — corrupt,
truncated, encrypted, or expanding past the 32 MiB decompression cap — is
`None`, which the indexer counts as skipped and walks past, exactly as it has
always treated an unreadable PDF. And a panic inside any extractor is caught at
this boundary, because these readers sit downstream of a decompressor and a
document somebody else wrote.

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
query ──┬─▶ prefix ──▶ embed ──▶ index.search(k*4) ──▶ chunk ids, by rank
        │                                                      │
        └─▶ terms ──▶ chunks_fts MATCH ──────────▶ chunk ids, by rank
                                                               │
                                              reciprocal rank fusion
                                                               │
                                                               ▼
                                                 SELECT ... WHERE c.id = ?
                                                               │
                                                               ▼
                                              Hit { score, path, lines, text }
```

Both halves of the store answer every query. The vector index knows what a
chunk means; FTS5 knows which literal terms it contains. Neither is sufficient
alone — an embedding of `EMBED_BATCH` sits in the same neighbourhood as every
other constant in the corpus, and a keyword index cannot answer "how does the
retry backoff work".

Embeddings are L2-normalized on both sides, which makes turbovec's inner product
equal to cosine similarity. That similarity is not comparable with BM25, though,
so the two rankings are fused by position rather than by score: each half
contributes `1 / (60 + rank)` and the sums decide the order. Reciprocal rank
fusion needs no calibration between two score distributions that have nothing to
do with each other, which is exactly the design problem that kept hybrid search
out of 0.1.0.

The reported score is therefore a fusion score, not a cosine. It is meaningful
for ordering within one result set and meaningless compared across queries.

Each half is searched `4 * k` deep before fusing, because a chunk ranked second
by one half and absent from the other still deserves consideration.

A query reaches FTS5 as bare terms, never as typed. FTS5's `MATCH` is a query
language: `AND` is an operator, `*` is a prefix wildcard, and an unbalanced
quote is a syntax error. Passing a user's words through raw would make
`index AND search` mean something they did not type and make `call_me(` fail
outright.

BGE English models were trained asymmetrically: passages are embedded raw, but
queries want an instruction prefix. `Model::query_text` adds it for those models
only. Omitting it measurably costs recall, which is why it is not a detail worth
simplifying away — and adding it for a model never trained with one, such as the
default, would be just as wrong.

If a chunk id comes back that SQLite does not know about, the hit is skipped
rather than failing the query. That means the two halves have drifted, which
should not happen — but returning four good results beats returning an error.

### Narrowing to part of the corpus

`--path`, `--ext` and `--lang` become one list of `GLOB` patterns, grouped so
that repeats within a kind union and kinds intersect. `src/filter.rs` owns that
translation; `store::filtered_chunk_ids` runs it as a single query against
`files.path` and returns the chunk ids it selects.

That one id set drives both halves. The vector half passes it to
`IdMapIndex::search_with_allowlist`, so turbovec masks the scan and its top-`k`
is computed *inside* the subset. The keyword half receives the same predicate
inside its FTS5 statement. Deriving the two independently would let them drift,
and fusion would then rank a chunk that one half was never allowed to return.

Filtering before the top-`k` rather than after is the whole point. A
subdirectory holding one percent of a corpus contributes roughly one percent of
a global top-8 — usually none of it — so post-filtering a global ranking
returns an empty result for exactly the query the filter was written for.

Three details are load-bearing:

- turbovec panics on an empty allowlist and on any id the index does not hold,
  so the ids are intersected with the index and the empty case returns no hits
  without calling it.
- A filter that ends up selecting the entire index is passed as no filter at
  all, which avoids building a mask the size of the index for no benefit.
- The unfiltered FTS5 statement is kept exactly as it was, with no join to
  `chunks` and `files`, so a query that uses no filter pays nothing for the
  feature.

Nothing is stored for any of this. `files.path` has been recorded since 0.1.0,
which is why filtering works on an existing store with no migration and no
re-embedding.

### Several stores, one query

`src/fleet.rs` opens the stores it is given and asks each of them the same
question. It is deliberately not a joint index: nothing is merged on disk, no
store learns about another, and every chunk id stays inside the store that
issued it. Ids collide across stores by construction — id 42 exists in all of
them — so an id that escaped its store would resolve to the right excerpt from
the wrong repository, which reads as a plausible answer rather than as a bug.

Three decisions carry the design.

**The query is embedded once per distinct model, not once per store.** The
embedder therefore lives in the fleet rather than in the store, and
`Semlith::search_ranked` takes a vector that has already been computed. Three
stores sharing a model cost one embed and one resident copy of the weights;
measured, an MCP server that has answered a query holds 137 MB whether it was
opened on one store or on three. A store
whose model differs is queried with its own model, because a vector from another
model is a point in a different space.

**Results are merged, not re-ranked.** Each store's list arrives already ranked
and that order survives the merge — a store's own answer is not up for
re-litigation by another store's numbers. Across stores the key is the fused
score, which is the one quantity that is the same unit everywhere: a sum of rank
reciprocals from the same formula at the same depth, in every store, under any
model.

**Ties are decided by similarity, not by argument order.** This is the part the
first test caught. Every store has a best hit whether or not it has an answer,
so a store whose top result is dense-rank-1 scores exactly what another store's
dense-rank-1 scores, and with two single-file stores the two collide exactly.
Ranking then fell to the order the stores were named in, which handed rank 1 to
a store that had nothing to do with the query. Ties now go to the higher
similarity to the query vector: it is the only evidence available about which of
two equally-ranked chunks is closer to what was asked, and it decides only
between hits the rank evidence has already called equal. Across two models it
compares numbers from two vector spaces, which is approximate — the worst case
is a reordering among equals, which is why a relevance floor was rejected. A
floor drops answers; this does not.

Two consequences elsewhere. Read commands go through `Semlith::open_existing`,
which refuses a directory that is not already a store, because `open` creates
what it is given and a mistyped store answers every question with nothing while
the other stores hide it. And the same store named twice is opened once,
deduplicated by canonical path: merging a store with itself gives every one of
its hits a twin at the same score and hands it the whole result list.

Writes are untouched. `index`, `watch` and `forget` take one store, because one
writer per store is a property of the store, not a limitation of the command.

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

`semlith_search` takes the CLI's filters as optional `path`, `ext` and `lang`
arrays, and a bare string is accepted wherever an array is, because that is what
an agent produces about half the time. An unknown language name and a filter
that selects nothing are both answered in-band with text the agent can act on:
one names `semlith languages`, the other says to try again without the filter.
Silently returning nothing would teach an agent that the corpus is empty.

The server serves a fleet, so one process can hold several stores. Two details
follow from the same principle: the open store names are written into the
`store` argument's description, because an agent cannot narrow to a name it has
never seen, and a name that is not open comes back as an in-band error listing
the ones that are. An empty result would be read as "the corpus does not discuss
this", which is a different and wrong answer.

## One writer per store

An index run holds an OS advisory lock on `index.lock` in the store directory
for its whole duration, including the final `index.tv` write.

The lock is the kernel's, not the file's existence. That distinction is the
whole point: a run killed with SIGKILL, or lost with the machine, releases the
lock when the process dies and leaves nothing to clean up. A lock file that
meant "locked because I exist" would wedge the store until a human deleted it,
and users would learn to delete it reflexively, which defeats the lock.

A second run does not wait. It exits non-zero naming the process that holds the
lock and when it started, because the honest options for a blocked indexer are
"wait an unknown time" or "tell the user"; the second is more useful from a
terminal and from a script.

Reads are not locked. A search during an index run sees whatever has been
committed so far, which is a consistent SQLite snapshot, and at worst misses
chunks that have not landed yet.

## Choosing the thread count

ONNX Runtime synchronises its threads at every operator boundary, so the batch
moves at the speed of the slowest thread. On a CPU where the cores are not
equal, that turns extra threads into a liability: a thread scheduled onto an
efficiency core holds up every thread on a performance core.

Measured on a 4P+4E Apple M1, indexing the same corpus:

| intra-op threads | chunks/sec |
|---|---|
| 1 | 5.1 |
| 2 | 14.0 |
| 4 | **16.5** |
| 8 | 13.9 |

So the default is the performance-core count on Apple silicon, and the total
core count everywhere else, where the cores are interchangeable and the whole
machine is the right answer. Undersubscribing costs far more than
oversubscribing — 1 thread is three times worse than 8 — so nothing else gets a
reduced count on a guess. `SEMLITH_EMBED_THREADS` overrides it, because this was
measured on exactly one machine.

Fanning batches across cores was measured and rejected: two workers with four
threads each managed 7.9 chunks/sec against 16.5 for one worker, and four
workers with one thread each managed 3.6. ONNX Runtime already owns the
machine; a second layer of parallelism only contends with it.

## Why these dependencies

| Crate | Why |
|---|---|
| [turbovec](https://github.com/RyanCodrai/turbovec) | TurboQuant is data-oblivious: no training, no rebuilds as the corpus grows. Add vectors, they are searchable. |
| rusqlite (bundled) | Bundled SQLite means no system dependency and one file to back up. |
| fastembed | Runs sentence-transformer models on CPU via ONNX Runtime, with model download and tokenization handled. |
| ignore | The `ripgrep` walker. Gets `.gitignore` semantics right, which is harder than it looks. |
| pdf-extract | Pure Rust, no external binary. |
| zip | Six of the document formats — `.docx`, `.pptx`, `.xlsx`, `.odt`, `.odp`, `.ods` — are ZIP archives of XML. Inflating one by hand is a decompressor, and that is not a thing to write. Taken with `default-features = false` and only `deflate-flate2`, so it is the reader and none of the compressors or ciphers. |
| blake3 | Fast enough that hashing every file on every run is free. |
| hf-hub | Fetches the default model's weights. fastembed uses it internally but does not expose it, and the default model is not one fastembed knows. |
| libc | `sysctlbyname` to count performance cores on Apple silicon, and the `SIGINT`/`SIGTERM` handler that stops `watch` at a batch boundary. |
| notify | Filesystem events per platform — FSEvents, inotify, ReadDirectoryChangesW — so `watch` costs nothing while nothing changes. Writing three backends by hand is not a thing to do for one command. |

## Watching, and why a reader can trust what it reads

`semlith watch` is not a second indexer. Filesystem events only produce a set of
candidate paths; the content hash, chunk eviction, batched embedding and the
atomic `index.tv` write are the same code `index` runs. Nothing is re-embedded
because an event fired — only because the bytes changed.

Two decisions carry the design.

**One writer, held honestly.** `Semlith` keeps its vector index in memory, and
`save()` writes all of it. Two writers would therefore not interleave, they
would overwrite: whichever saved last would erase the other's work. So `watch`
takes the store lock for its whole life and a concurrent `index` is refused by
name. A long-held lock is the visible cost of an invariant that was already
there.

**Freshness is a counter, not a timestamp.** A reader — an MCP server an agent
holds open for a session — must notice that the index has been replaced. The
store counts index rewrites in its `meta` table, bumped *after* the rename, and
a search reloads when the count has moved. mtime cannot do this job: re-embedding
one file can leave both the file size and a second-granularity mtime unchanged,
and the reader would go on answering from vectors that no longer exist. Bumping
after the rename is what makes the counter safe to trust — a reader that sees
the new generation is guaranteed to find the new index behind it.

Deciding what an event means is deliberately postponed to the moment the batch
is indexed, and answered by the filesystem rather than by the event kind: a path
that exists is re-embedded, a path that does not is evicted. Renames and
write-temp-then-rename saves then need no special case, and the differences
between how FSEvents, inotify and ReadDirectoryChangesW label things stop
mattering.

## Things that were considered and left out

**A daemon or server mode.** Query latency is a few milliseconds warm on a
repository-sized store; the only cost worth amortizing is model load, which
`semlith mcp` already does. A network port
would also undermine the "everything local" property that is the entire point.

**Storing embeddings in SQLite too.** Tempting for a single-file store, but then
every query either scans blobs out of SQLite or duplicates them in memory.
Keeping vectors in a purpose-built index is what makes the query fast.

**Per-file license headers.** Apache-2.0 recommends but does not require them,
and the Rust ecosystem convention is the `license` field in `Cargo.toml` plus a
`LICENSE` file. Both are present.
