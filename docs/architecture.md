# Architecture

How semlith is put together and why. Read it before changing the code; it is the
context the code assumes. Measured numbers are in
[performance.md](performance.md), stability promises in
[compatibility.md](compatibility.md).

The design goal: **indexing can be slow, querying must not be.** Turning text
into vectors is the expensive part, and it happens once per chunk at index time.
A query is one embedding plus a scan.

## The store

```
~/.semlith/
├── registry.json                 which store covers which roots, and the model each was built with
├── agent.key                     the credential a client presents to the daemon
└── stores/
    └── semlith/                  one store, named for what it covers
        ├── index/                turbovec shards — quantized vectors, keyed by chunk id
        │   └── 0000000000000001.tvim
        ├── images/               the same again at CLIP's 512 dimensions, only once a store has met an image
        │   └── index/
        │       └── 0000000000000001.tvim
        ├── store.db              SQLite — chunk text, file paths, line spans, symbols, edges, content hashes
        ├── index.lock            the OS advisory lock one writer holds for a whole run
        └── daemon.json           written while a daemon holds the lock, so `semlith mcp` can forward to it
```

A store kept beside its corpus is the same directory named `.semlith/`.

**Vectors and text live apart.** The vector side holds only vectors and ids,
never text, because the vectors are scanned on every query and their size
decides how much must be resident. At 4 bits per coordinate and 384 dimensions a
chunk costs 192 bytes, so a million chunks is about 190 MB of packed codes,
while the text can be gigabytes sitting in SQLite.

**The index is sharded.** `index/` holds fixed-size shards of 65536 vectors
(`index::SHARD_VECTORS`), each named for the first chunk id it holds and
zero-padded so sorted-by-name is sorted-by-id. A shard is about 12 MB of packed
codes, doubled by turbovec's repacked search copy. Only as many shards are held
open as the memory budget allows; the rest are read back when a query reaches
them. **The directory listing is the manifest**: nothing else records shard
boundaries, so nothing can disagree with it. A store written before 0.7.0 keeps
a single `index.tv` and is never migrated (see
[compatibility.md](compatibility.md#store-format)).

**One integer joins the halves.** A chunk's SQLite rowid is its turbovec
external id. There is no mapping table.

Beside the shards, `exact.f32` keeps each vector at full precision, so a query
can rescore its candidates by the true vectors rather than the 4-bit codes.

## Indexing

```
 walk ──▶ recently changed first ──▶ paths
                                      │
      ┌───────────── prepare pool (one thread per performance core) ─────────┐
      │ read ─▶ hash ─▶ unchanged? ─▶ skip                                  │
      │          │                                                           │
      │          ▼                                                           │
      │   extract text ─▶ secret scan ─▶ parse (tree-sitter) ─▶ chunk_file() │
      │          │                                                           │
      │          ▼                                                           │
      │   vector cache? ─▶ hit: the vector      miss: tokenize, once          │
      └──────────────────────────────┬───────────────────────────────────────┘
                                     ▼  (in walk order)
      writer: INSERT INTO files/chunks (one transaction per window) ──▶ ids
                                     │
                                     ▼  windows of 256 chunks, rows committed
      embed stage: token-budget batches ──▶ Neural Engine │ GPU │ CPU │ …
                   two in flight per lane; every vector checked finite, non-zero
                                     │
                                     ▼  whole windows, in id order
      writer: add_with_ids + exact.f32 ──▶ checkpoint: write dirty shards ──▶ commit hashes
```

A pass is three stages running at once (`src/pipeline.rs`):

- **Prepare.** Reading, hashing, extraction, the secret scan, parsing, chunking
  and tokenising need no database. They run on a pool sized from the performance
  cores and hand files back in walk order, so a run stays deterministic. A chunk
  is tokenised once; lanes receive token ids.
- **Write.** The writer is the only thread that touches the store, and it never
  runs a model: it commits rows and lands vectors.
- **Embed.** Each window is sorted by token count. The shortest chunks go to the
  CPU and the longest to an accelerator. Every batch is sized by padded tokens
  from the lane's own measured pace (about 0.2 s of work), and each lane keeps
  two batches queued so a device never waits.

Splitting the stages fixed a measured stall: with one thread running the CPU
model inline, that model took 64 % of a run while the GPU lane waited for
batches 28 % of the time. The run's wall time is reported by stage — walk, read
and hash, extract and scan, parse and chunk, tokenize, each lane's wait, and
write — in the run snapshot, the daemon log and `semlith index --verbose`, and
the parts add up to the whole.

**Files are hashed first.** BLAKE3 over the bytes, compared with the hash
recorded last time. An unchanged corpus re-indexes in milliseconds, and the
model is never loaded on a no-op run.

**Hashes are committed last, after the shards are on disk.** A file's row is
inserted with an empty hash while its vectors are in flight. If the process dies
mid-run, those files still have an empty hash and are re-indexed next time.
Recording the hash first would leave chunks no vector points at, silently
unsearchable.

**Changed files are evicted before they are re-added.** `delete_file` returns
the old chunk ids so they leave the vector index with the SQL delete. SQLite
reuses rowids, so skipping this would eventually give an old vector a new
chunk's id. A file emptied on disk, grown past the size cap or made unreadable
is evicted the same way.

**Each shard is written to a temp file and renamed**, so an interrupted save
never leaves a truncated shard. Only shards the run touched are rewritten. This
is not all-or-nothing: a process killed mid-save leaves some shards new and some
old. The hashes are the crash-safety story — files whose vectors were in flight
have no hash, so the next run re-indexes them and rewrites exactly those shards.
A leftover `.tmp` is removed on the next open by a caller holding the store lock,
which therefore knows no live writer owns it.

**A store answers while it fills.** Rows are committed a window at a time,
before the window is embedded, and readers have their own connections, so
keyword and graph search answer for every file already written. The writer may
hand the embed stage up to sixteen windows beyond the three it works on, so a
cold run writes thousands of chunks' rows in its first second. The most recently
changed files (by modification time or by the last commit that touched them) go
first. A pass over roots does not wait for the walk: it starts with the files
the last commits touched in every repository up to two folders below each root,
code before licences and changelogs within one commit, while the walk continues
beside it. A semantic query mid-run embeds up to sixteen of its best keyword
matches that have no vector yet, merges them into the vector list by
similarity, and says how much of the store is pending; an identifier-shaped
query trusts the keyword half and skips that embed.

**The vector cache.** The same chunk text under the same model, variant,
chunking rules and truncation always gets the same vector, so the machine keeps
them in one SQLite file under the semlith home (`cache/vectors.db`), bounded by
a least-recently-used cap. The prepare stage looks a chunk up before tokenising
it, and a hit never reaches a lane. An edit to one function re-embeds only that
function's chunk; a second worktree of a repository re-embeds almost nothing.
The cache belongs to the machine, not a store: compaction does not touch it,
eviction does not touch a store, and deleting it costs a re-embed and nothing
else.

## Extraction

`chunk::extract` dispatches on the extension before it reads a byte. A `.docx`,
`.pptx`, `.xlsx` or OpenDocument file is a ZIP archive, so the NUL-byte check
that rejects binaries would reject every document if it ran first, and a corpus
of ordinary source pays one string comparison per file.

The readers live in `src/formats.rs`, private because what is extracted from a
document is behaviour, not API. Six of the nine formats are ZIP archives of XML
and share one bounded archive reader and one tag scanner. A notebook is JSON,
handled by serde_json. HTML is a character scan that removes tags but keeps every
newline, so a hit in an HTML page still names the line in the file on disk.

A file that cannot be read — corrupt, truncated, encrypted, or expanding past
the 32 MiB decompression cap — is `None`, counted as skipped. A panic inside any
extractor is caught at this boundary, because these readers sit downstream of a
decompressor and a document somebody else wrote.

## Chunking

Chunks are line-aligned and capped at 800 characters (`chunk::MAX_CHARS`), with
the last two lines repeated into the next chunk (`OVERLAP_LINES`).

**A chunk starts where the file has structure.** A source file is cut at the
definitions tree-sitter found for the graph, each extended upwards over its doc
comment and attributes; a Markdown file is cut at its headings. A fixed window
split `MAX_NODES` from the six lines explaining it, so neither chunk answered a
question about it. The budget still ends a chunk, so a long function is split;
what a cut guarantees is that a definition never begins mid-chunk. The two-line
overlap is dropped where the next chunk starts on a cut, because a definition's
first line is not an arbitrary boundary.

**A Markdown chunk carries its heading path** (`Architecture > Chunking`) in
`Chunk::context`, not in its text. The model is shown the path in front of the
chunk; the store keeps the file's own bytes, because `Semlith::read` maps every
line of a chunk to `start_line + offset` and an invented line would shift every
span. A code chunk is likewise embedded with the definition it sits inside.

**800 characters is measured.** Transformer cost grows faster than linearly with
sequence length: going from 1200 to 800 characters improved throughput 1.58x per
chunk and cut peak memory by 0.5 GB. Smaller chunks also retrieve more precisely
and cost an agent fewer tokens.

Line alignment is what makes the `path:start-end` locator work. A line longer
than the whole budget (minified JavaScript, embedded base64) is hard-split on a
character boundary, and every piece shares its line number.

The chunking rule is a store format: a store records which rule cut it,
`semlith stats` shows it, and only a pass that sweeps the whole store moves the
format row ([compatibility.md](compatibility.md#store-format)).

## Searching

```
query ──┬─▶ embed ──▶ index.search(k*4) ──▶ exact rescoring ──▶ vector list
        ├─▶ terms ──▶ chunks_fts MATCH ─────────────────────▶ keyword list
        ├─▶ names in the query ──▶ symbols ──────────────────▶ definition / named lists
        └─▶ CLIP text encoder (only if the store holds images) ▶ image list
                                                    │
                                     reciprocal rank fusion
                                                    │
                       lifts, demotions, optional cross-encoder, copy collapse
                                                    ▼
                                    Hit { score, path, lines, text, lists }
```

The vector index knows what a chunk means; FTS5 knows which literal terms it
contains. Neither suffices alone: an embedding of `EMBED_BATCH` sits near every
other constant, and a keyword index cannot answer "how does the retry backoff
work". The vector and keyword lists are each searched `4 * k` deep before
fusing, so a chunk ranked second by one and absent from the other is still
considered.

Embeddings are L2-normalised on both sides, so turbovec's inner product is cosine
similarity. Candidates are then reordered by their full-precision vectors from
`exact.f32`.

A query reaches FTS5 as bare terms, never as typed. `MATCH` is a query language:
`AND` is an operator, `*` a wildcard, and an unbalanced quote a syntax error.

BGE English models want an instruction prefix on queries but not on passages.
`Model::query_text` adds it for those models only; omitting it measurably costs
recall, and adding it to a model not trained with one is just as wrong.

If a chunk id comes back that SQLite does not know, the hit is skipped rather
than failing the query: the halves have drifted, which should not happen, but
four good results beat an error.

### Ranking

**Reciprocal rank fusion.** Cosine and BM25 are not comparable, so lists are
fused by position: each contributes `weight / (k + rank)` and the sums decide
the order. The default `k` is 60 (`RRF_K`). The reported score is therefore a
fusion score, not a cosine: it orders one result set and means nothing across
queries.

**A query is read before it is ranked.** `shape_of` (in `lib.rs`, the one
classifier; the portal reads the shape from the server's reply) calls a single
token of identifier characters an identifier and anything else a question. The
list that shape trusts gets a steep curve, `k` = 12: the keyword list for an
identifier, the vector list for a question. A flat curve let two vague lists
placing a chunk fourth and fifth outvote one authoritative list placing another
first. The image and named lists are always steep. The shape is reported in
every answer, and `prefer` overrules it.

**The lists:**

| List | When | What it adds |
|---|---|---|
| vector | always | chunks by meaning |
| keyword | always | chunks by literal terms (FTS5) |
| definition | identifier-shaped query | up to three chunks defining that exact name (`DEFINITION_LIFT`), lifted above the fused order rather than scored into it; the badge `definition` |
| named | question-shaped query | the definitions of identifiers the sentence names, as a list that votes beside the others |
| image | the store holds images | images by CLIP similarity, keyed in their own id space; a confident match is weighted to stand in for the lists a chunk can appear in, and a query CLIP had to cut short (over 77 tokens) is never confident |

**After fusion**, applied once the rows are fetched because each needs something
fusion cannot know:

- a project prior on a store of several projects: a hit is multiplied by
  `1 + 1.0 × share`, where `share` is its project's part of the head's fused
  score, and by a further `1 + 3.0` when the query names its project (a word of
  four letters or more from the project directory's name);
- a 10 % demotion for a stale hit (the file changed since indexing);
- a 10 % demotion for a span under `tests/`, `test/`, `fixtures/`, `spec/` or
  `__tests__/`, unless the question names tests;
- `prefer: code` or `prefer: docs` multiplies the preferred side by 1.5. It is a
  bias, not a filter. With no `prefer`, each store's `lean` setting applies;
- the optional cross-encoder (`SEMLITH_RERANK=on`, not for identifier queries)
  reorders the head, up to twelve hits, reading each hit's first 320 characters
  ([models.md](models.md#rescoring-jina-reranker-v1-turbo-en));
- hits whose text is byte-identical collapse into the best-ranked one, which
  names the others as `also in N copies`.

No weight is learned; every input is something the store already holds.

**Search does not use the graph.** Until 0.36.0 a third list walked the code
graph from the top hits (a personalised PageRank, damped at 0.85, three rounds).
On the 727-question benchmark of 2026-10-01 search scored the same without it
(398 against 397 of 507), while it took 112 of 328 ms at the median on the
879k-chunk corpus. `neighbors`, `impact`, `path`, `trace` and brief's callers and
callees still read the graph.

**A lift for chunks inside a named definition was built and removed.** It cost
two hits at k=3, and in a code repository nearly every code chunk sits inside a
definition and nearly no prose chunk does, so it acted as an always-on
`prefer: code`, overriding a caller who asked for docs.

### Narrowing to part of the corpus

`--path`, `--ext` and `--lang` become one list of `GLOB` patterns: repeats of a
kind union, kinds intersect, and a leading `!` excludes. `src/filter.rs` owns the
translation; `store::filtered_chunk_ids` runs it as one query against
`files.path`. A scope with a literal directory in front (`<root>/**`) is a range
over an index on the lowered path instead of a GLOB over every path. A store
keeps its eight most recently resolved filters.

That one id set drives every list. The vector list passes it to
`IdMapIndex::search_with_allowlist`, so the top-`k` is computed inside the
subset; the keyword list gets the same predicate in its FTS5 statement; the
definition and named lists resolve through `store::symbols_by_names` with the
same filter. Deriving them independently would let fusion rank a chunk one list
was never allowed to return.

Filtering before the top-`k` is the point. A subdirectory holding one percent of
a corpus contributes roughly one percent of a global top-8, so post-filtering
returns nothing for exactly the query the filter was written for.

- turbovec panics on an empty allowlist and on an id it does not hold, so ids
  are intersected with the index first and an empty set returns no hits.
  Allowlists are sorted once, so each shard takes its ids by binary search.
- A filter selecting the whole index is passed as no filter.
- The unfiltered FTS5 statement has no join to `chunks` and `files`, so a query
  without a filter pays nothing.

Nothing is stored for filtering; `files.path` has always been recorded, so it
works on any store without migration.

### Several stores, one query

`src/fleet.rs` asks each open store the same question. It is not a joint index:
nothing is merged on disk, and every chunk id stays in the store that issued it.
Ids collide across stores by construction, so an escaped id would resolve to the
right excerpt from the wrong repository.

- **The query is embedded once per distinct model, not per store.** The embedder
  lives in the fleet, and `Semlith::search_ranked` takes a precomputed vector.
  Three stores sharing a model cost one embed and one copy of the weights. A
  store with a different model is queried with its own, because its vectors live
  in a different space.
- **Results are merged, not re-ranked.** Each store's order survives. Across
  stores the key is the fused score, the one quantity in the same unit
  everywhere.
- **Ties go to the higher similarity to the query vector**, not to argument
  order. Every store has a best hit, so two stores' rank-1 hits can tie exactly;
  ordering by argument once gave rank 1 to an unrelated store. Across two models
  this compares two vector spaces, which is approximate, but it only reorders
  equals. A relevance floor was rejected because it drops answers.

Read commands use `Semlith::open_existing`, which refuses a directory that is
not already a store: `open` creates what it is given, and a mistyped store would
answer nothing while the others hid it. A store named twice is opened once,
deduplicated by canonical path. Writes (`index`, `watch`, `forget`) take one
store, because one writer per store is a property of the store.

A connected Semlith Cloud store is merged by score into a search that names it,
or into any search while remote stores are connected; other tools naming one
forward the call, and writes to one are refused.

### Reading one span

A locate answer tells an agent where to look; `semlith read` (`semlith_read`)
returns exactly that span, by `path:start-end`, `path:line` or symbol name, so
the agent does not read the whole file.

- **It answers from what was indexed.** A path never indexed is not read, so
  naming `~/.ssh/id_rsa` cannot get around the deny-list. A file edited since
  indexing is read from disk, scanned, redacted if it was accepted that way, and
  marked.
- **It stitches by line number.** Chunks overlap by two lines, so concatenating
  them would repeat the seam and shift every later line number.
- **It resolves a path by suffix** against what was indexed. Two files matching
  one suffix is an error naming both, not a guess.
- A name with up to four definitions returns each whole, capped at 32 000
  characters.

## The MCP server

`semlith mcp` is newline-delimited JSON-RPC 2.0 over stdio, hand-rolled in
`src/mcp.rs`. A tools-only server needs `initialize`, `tools/list`, `tools/call`
and `ping`, which is less than a dependency would cost. When a daemon is
running, `semlith mcp` forwards to it; otherwise it opens the stores itself. The
daemon serves the same protocol over HTTP at `/mcp`.

- **stdout is protocol.** Nothing else writes there, which is why `Semlith` has
  a `quiet` flag: a download progress bar would corrupt the stream.
- **Requests without an id are notifications** and get no answer
  (`notifications/initialized` is the common case).
- **Tool failures are returned in-band** as `isError` content, so the agent sees
  what went wrong.
- **The server warms up at start** (`warm()` loads the model and prepares the
  index caches), so the first call is not hundreds of milliseconds slower.

`tools/list` advertises eight tools (`mcp::LISTED`: search, brief, read, files,
symbol, neighbors, impact, stats) to keep the per-session cost small; the server
answers all sixteen, and `SEMLITH_MCP_TOOLS=all` lists them all. Every array
parameter declares `items`, which Copilot in VS Code requires.

**Search answers where, not what.** `format: locate` (the MCP default) returns
one line per hit — `start-end name kind @line · lists | best line` — grouped by
file under a `root …` line that names the store root once, with paths relative
to it, cut to a `max_tokens` budget that states `truncated: N of M`. Sending
excerpts back cost 20 to 60 times what the grep it replaced would have.
`format: "excerpt"` returns text; the CLI prints excerpts, because a person at a
terminal is not paying by the token.

**Freshness** is one `stat` per distinct path, comparing size and mtime with what
the store recorded. It is conservative — a `touch` reads as stale — because a
false "check this" costs a reread and a false "current" costs a wrong quotation.

**Arguments are forgiving and errors are actionable.** `path`, `ext` and `lang`
accept a bare string where an array is expected, because agents produce one half
the time. An unknown language names `semlith languages`; a filter that selects
nothing says to retry without it; a store name that is not open lists the ones
that are. Open store names are written into the `store` argument's description,
because an agent cannot narrow to a name it has never seen. An empty result
would read as "the corpus does not discuss this", which is a different answer.

The `initialize` reply's `instructions` (at most 600 bytes) name the indexed
folders, so a client routes questions about them to semlith.

## One writer per store

An index run holds an OS advisory lock on `index.lock` for its whole duration,
including the final shard writes. The lock is the kernel's, not the file's
existence: a run killed with SIGKILL, or lost with the machine, releases it and
leaves nothing to clean up. A second run does not wait; it exits non-zero naming
the holder and when it started.

Reads are not locked. A search during a run sees a consistent SQLite snapshot of
what has been committed.

## Watching

`semlith watch` is not a second indexer. Filesystem events produce candidate
paths; hashing, eviction, embedding and the per-shard write are the code `index`
runs. Nothing is re-embedded unless its bytes changed. What an event means is
decided when the batch is indexed, by the filesystem rather than the event kind:
a path that exists is re-embedded, one that does not is evicted. Renames and
write-temp-then-rename saves need no special case, and FSEvents, inotify and
ReadDirectoryChangesW labels stop mattering.

**One writer, held honestly.** `save()` rewrites each dirty shard whole, so two
writers would overwrite each other's work. `watch` takes the store lock for its
whole life and a concurrent `index` is refused by name.

**Freshness is a counter, not a timestamp.** A long-lived reader must notice
when the index changes. The store counts index rewrites in its `meta` table,
bumped after the rename, and a reader reloads when the count moves. mtime cannot
do this: re-embedding one file can leave size and a second-granularity mtime
unchanged. A reader re-opens its indexes when the generation moves, because the
shard list is read when an index opens.

**Compaction swaps the whole set.** An incrementally updated store only grows,
so `compact` rewrites `exact.f32` to the live records and rebuilds the shards
packed full. The codes are the same bytes, because turbovec's encoding depends
only on the values. The new set is built beside the live one and put in by two
renames; the store is marked for the swap and the generation moves after it, so
a search that overlaps the mark or a generation move is asked again against what
is on disk.

## The daemon

`semlith start` runs one process that owns every store's writer, serves the
portal, and answers MCP at `/mcp`. It binds `127.0.0.1` only (see
[security.md](security.md)). What it buys is not query latency — that was already
a few milliseconds warm — but the end of the one-writer conflict: a watcher
holding a store's lock used to block an agent's `semlith_index` for as long as
it ran. One process as the writer, with everything else its client, is the only
fix that keeps the rule that keeps the vector index and SQLite agreeing.

**Priority follows the work.** One process-wide count covers every embed pass,
query embedding and HTTP request. On its 0→1 edge the daemon clears its darwin
background state (`setpriority(PRIO_DARWIN_PROCESS, 0, 0)`); 300 ms after it
returns to 0 it sets it again. The launchd plist asks for `ProcessType Standard`
(`Background` pinned it to the efficiency cores). Windows does the same with
`SetPriorityClass` and EcoQoS power throttling. Linux is left alone, because an
unprivileged process cannot lower its nice value once it has raised it; the
systemd unit sets `Nice=5`, `CPUWeight=50` and `IOWeight=50`. A launchd agent's
threads still run at priority 20 against 31 from a terminal, which is why the
service indexes at about 60 % of terminal speed on a Mac.

**One queue for everything that embeds.** A portal run, a watcher catch-up and
a watcher batch of more than 32 files are admitted through one daemon-wide queue,
so *runs at once* is a real ceiling. Lowering it holds the newest runs at their
next batch as `held`; they keep their progress and resume oldest first. A batch
of 32 files or fewer runs at once, because a saved file must be searchable
within seconds.

**Control at every batch.** The pass checks its control before every embedding
batch, inside a file as well as between files, so pause and stop act within one
batch. Stop undoes what the run wrote and saves the index once; with its delete
box ticked, the store's writer, watcher and readers close before the directory
is renamed aside and removed.

**Memory follows the work.** ONNX Runtime's arenas never shrink, so a writer
drops its session after 60 s without embedding (`SEMLITH_SESSION_IDLE_SECS`) and
a GPU worker exits after 60 idle seconds. Readers share one query session per
model. A reader panic during extraction fails that file, not the writer.

### Embedding lanes

The CPU lane runs int8 in the daemon. Every other lane is a worker process
(`semlith __embed-worker <lane>`) speaking length-framed batches on stdin and
stdout, so a driver crash ends that worker, not the daemon. A worker that exits,
errs or misses its per-batch deadline fails its own lane; its batch goes back
and the run finishes on what is left. A failed lane is tried again on the first
run ten minutes later, and the failure is logged. Before its first batch a lane
embeds 32 committed chunks and must agree with their fp32 vectors at cosine
0.999 or better. A software renderer (lavapipe, WARP, SwiftShader) is refused by
vendor and name before anything is downloaded. Which lane embedded a chunk
depends on timing, so two hybrid runs give the same chunks but not bit-identical
vectors; the store counts chunks per variant in the `variants` meta row.

| Lane | Runs | Notes |
|---|---|---|
| CPU | int8 in the daemon | always on; capped by `SEMLITH_CPU_CAP` or the saved cap |
| GPU | fp16 through Microsoft's WebGPU plugin, or Core ML on Apple silicon | |
| Neural Engine (`ane`) | granite as native Core ML models | Apple silicon |
| CUDA, TensorRT for RTX, OpenVINO, llama.cpp | vendor plugins or llama.cpp's server | experimental |

**The lane policy.** On Apple silicon the Neural Engine goes first and nothing
else embeds beside it: two int8 CPU threads cut it from 236 to 79 chunks/s by
taking the cores that tokenise and feed it, and the GPU beside it adds 30 % in
bursts but 4 % sustained on a fanless Air. `gpu-beside-ane` turns the GPU back
on for a machine that can cool both. Elsewhere every accelerator that is on runs
with the CPU beside it. A lane starts in the background and takes batches once
its worker passes the known-answer check; until then, and after any failure, the
run continues with what it has.

**The Neural Engine through Core ML.** ONNX Runtime's Core ML provider places
almost nothing of granite on the Neural Engine; a native Core ML model does.
`packs/coreml/` re-implements granite in the Neural Engine's layout — `(B, C, 1,
S)` tensors, 1x1 convolutions instead of linear layers, attention per head — and
converts it at fixed shapes: four rows, six length buckets from 128 to 512
tokens. On the M1, 3 375 of its 3 380 ops run on the Neural Engine, at 236.5
chunks/s sustained against the CPU's 30.7, with every vector at cosine ≥ 0.9999
against fp32. Two changes made fp16 work: a LayerNorm that squares its input
overflows, so it squares a sixty-fourth of it; and a padding row keeps one key
unmasked so its softmax never divides by zero.

CI builds the models from that directory, publishes them as a pack on this
repository's release, and `src/packs.rs` pins them by digest
([models.md](models.md#the-core-ml-pack)). macOS compiles a model for the
machine's Neural Engine on first load — 30 to 43 s a bucket on the M1, one at a
time in a single system compiler service — and caches the result under the
loading program's name, so a new binary would compile all six again. The Core
ML lanes therefore run from a copy of semlith under `accel/coreml-worker-v<N>`
in the model cache, made once per worker protocol (`COREML_WORKER`) and kept
across upgrades. A session is ready once its longest bucket has loaded (39 s
cold, about 2 s cached) and loads the other five on its own thread at
user-request priority (27 s a bucket against 44 s at a spawned thread's
default). A compile lock keeps the worker from being retired mid-compile and
keeps a second semlith from queueing the same compile: the compiler service
works through a killed program's requests anyway, and a backlog once kept a lane
compiling for over two hours. The daemon deletes half-written compiles a killed
worker leaves (about 94 MB each), and the last compile's time is the next one's
estimate. The lane shows compiling with a percentage and time left, and a run
with no other lane on and the CPU off waits for it.

**Lanes checked without their hardware.** TensorRT for RTX, OpenVINO and
llama.cpp are built and checked without their devices. NVIDIA and Intel publish
their execution providers as PyPI plugins that load into the worker's ONNX
Runtime, as Microsoft's WebGPU plugin does; llama.cpp runs its own server on
`127.0.0.1` with a key only its worker knows. CI proves each builds, loads, says
why it is unavailable without its device, falls back, and — where a CPU path
exists (OpenVINO's CPU device, llama.cpp's CPU backend) — gives the known
answer. No throughput is claimed for them or for CUDA, and each is labelled
experimental.

## Choosing the thread count

ONNX Runtime synchronises its threads at every operator, so a batch moves at the
speed of the slowest thread, and a thread on an efficiency core holds up every
thread on a performance core. Measured on a 4P+4E M1, indexing one corpus:

| intra-op threads | chunks/sec |
|---|---|
| 1 | 5.1 |
| 2 | 14.0 |
| 4 | **16.5** |
| 8 | 13.9 |

The default is the performance-core count on a hybrid CPU (Apple silicon via
`sysctlbyname`; Intel P-cores from `/sys/devices/cpu_core/cpus` on Linux and the
highest `EfficiencyClass` on Windows), and the total core count elsewhere. In the
daemon the count is split among the runs embedding at that moment: a run alone
gets all of it, and each run rebuilds its session at its next batch when another
starts or ends. A saved *threads each* is used as given, and the CPU cap bounds
it. Undersubscribing costs more than oversubscribing (1 thread is three times
worse than 8), so nothing gets a reduced count on a guess.
`SEMLITH_EMBED_THREADS` overrides it; pinning it also makes embedding
reproducible, because ONNX Runtime reduces across threads in whatever order they
finish.

Fanning batches across workers was measured and rejected: two workers with four
threads each managed 7.9 chunks/sec against 16.5 for one, and four workers with
one thread each managed 3.6. ONNX Runtime already owns the machine.

## The code graph

### Where extraction happens

Symbol extraction is spliced into `Semlith::index_set`, after the old rows are
deleted and while the file id and new chunk ids are in hand. Nothing else calls
it. The pass that re-embeds a file is the pass that re-extracts it, so `index`,
the watcher and the daemon all inherit it through one code path, and no build
artifact can drift from the corpus. There is no `semlith graph build`, on
purpose.

### Tables

```
symbols(id, file_id -> files.id CASCADE, chunk_id, kind, name, qualified, start_line, end_line)
edges(src -> symbols.id CASCADE, dst TEXT, kind, confidence, hint TEXT)
```

An edge belongs to the file its **source** is in and dies with it. Its **target
is a name**, resolved through `symbols(name)` at query time. Symbol ids are
reissued on every re-extraction, so an id in `dst` would mean re-indexing `b.rs`
silently deleted every edge into it from `a.rs`. By name, an edge is as current
as both its ends, and an edge to something outside the corpus (a
standard-library call) is still recorded and resolves to nothing.

### Hints and confidence

A bare name is not an address: any store holds several `get`s and `index`es, and
resolving by name alone once made `semlith path` answer yes when the honest
answer was no.

**`edges.hint` records what the source said**: the module of a scoped call
(`store::edges_out` gives `store`), the receiver's last identifier
(`self.index.search` gives `index`), the object of a qualified Python or
TypeScript call. For Rust it is the receiver's type where the source states it:
`self` inside `impl T`; a parameter or local declared `x: T` (through `&`,
`&mut`, `Box`, `Rc`, `Arc` and the lock types); a local bound to `T::new()`,
`T::open(…)?` or `T { … }`; a field declared `store: T` in the same file. A
Rust method's `qualified` name records its owner (`Fleet::search_preferring`).
Rust's scoped-call path segment is not emitted as a second `calls` edge:
`store::edges_out()` is a call to `edges_out`, not to `store`.

**The query decides the ranking.** `store::edges_out` prefers, in order: a
candidate whose owner is the hinted type; a definition in the same file as the
call; one whose file the hint names; one in a file the calling file imports; and
failing those, the only definition of the name in the corpus. A remaining tie is
broken by the number of directories each candidate shares with the caller, so a
call in `src/mcp.rs` means `src/fleet.rs` and not a fixture copy; a tie nothing
places stays a tie. A call to the same name on a receiver of another type is not
dropped as recursion.

**Confidence has four values, two stored.** `extracted` and `inferred` are
written at extraction time; `resolved` (one survivor) and `ambiguous` (several,
each carrying the count) are computed at query time and never stored, because
re-indexing a target changes which definitions exist. An edge the syntax tree
already settled stays `extracted` — including a call whose hint the file
imports. The import list is read lazily, per source file, only when the first
tiers did not settle a group. Inbound rows are found through the edge's own
`src` id, so `edges_in` reports the stored value.

### Where the queries come from

Some grammars ship a `TAGS_QUERY`. It is good for definitions and uneven for
references: most capture no imports, and TypeScript, C, C++, C#, JavaScript, PHP
and Swift capture no calls. Those pair it with a supplement in `graph.rs`, and
matches are read whole, because a tags query spans `@name` separately (Java puts
`@reference.call` on the argument list). Most grammars ship no tags query; theirs
live in `queries/<language>/tags.scm`, in the same capture vocabulary
(`@definition.*`, `@name`, `@hint`, `@reference.*`), and are `include_str!`'d.
Queries are compiled once per process and cached.

A bundled query can be wrong about a span. C and C++ tag the declarator, so a
function's range stopped at its signature and calls in its body were attributed
to the file. The supplement spans the whole `function_definition`, and `extract`
drops a definition contained by another of the same name and kind (a Rust `mod
x` holding an `fn x`, or a Java class and its constructor, keep both).

**Languages without functions.** Fourteen of the forty-six languages are markup,
data or configuration. Their symbols are their structure — a YAML, TOML or JSON
key, a Markdown heading, a Terraform block label, a Dockerfile stage, a Makefile
target, a GraphQL type, a protobuf message or RPC, a SQL table or view, a CSS
selector, an HTML element with an id — each with a kind. Their references are to
files: an `include`, `import`, `source`, `FROM`, a stylesheet or script `src`, a
Makefile prerequisite, the table a view selects from. The test for extracting a
key is whether a developer would ask about it. Svelte and Vue grammars hand the
`<script>` block back as raw text, so a component's methods are not symbols; the
template's structure is.

### Traversal

There is no graph library and no in-memory graph. `neighbours`, `shortest_path`
and `impact` walk the indexed `edges(src)` and `edges(dst)` columns one hop at a
time, bounded by a depth limit and `graph::MAX_NODES`. A name already seen is not
expanded twice, so cycles terminate. Peak RSS stays flat as the corpus grows
(`tests/measure.rs` asserts it), and a truncated answer says so.

`impact` and `path` follow `calls`, `imports` and `references` only. `defines`
and `contains` are true but would make every symbol one hop from its file, so
any blast radius would be at least the whole file.

**A path walks definitions, not names.** A node is a definition (name, file,
line), and a chain may leave only from the definition it arrived at;
`graph::Step` carries both endpoints. Otherwise every hop can be true and the
chain false — arriving at `search` in `lib.rs` and leaving from `search` in
`routes.rs`. The default finder does not cross a name with several definitions
and answers "not connected within N hops by resolved edges", because a wrong
chain reads exactly like a right one. `--all-edges` (`all_edges: true`) walks by
name and labels it: both endpoints on every hop, a seam wherever a hop leaves a
different definition than the last arrived at, a trailer whose four confidence
counts add up to the hop count and names the ambiguous names crossed, and the
line "A hypothesis, not a finding." `--strict` (`strict: true`) states the
default explicitly and wins when both are given. One renderer serves the CLI and
MCP.

**`neighbors` collapses ambiguity.** Callees to a name with several definitions
become one row with the count (`get · ambiguous · 4 definitions`). `--all`
(`all: true`) expands them and lists targets the store has no definition for.

**`impact` is `shortest_path` read backwards**, breadth first over
`store::edges_in` under the same two rules, so each caller is reported at its
fewest hops. Each hop asks what calls *this definition*, through the same
resolver. `IMPACT_LIMIT` bounds the answer and the count left out is reported.
Answers from `impact`, `neighbors` and `symbol` are capped at 16 000 characters;
rows past the cap collapse into per-file counts with a `more:` line. They take
`Type::method`, `module::function` and `Type.method`.

**`trace`** takes a chain the finder produced and reads one line of source per
hop from the store's chunks, so the Trace panel and `semlith path` cannot
disagree. A hop is a supporting fact if its edge is `extracted` or `resolved`,
and a candidate if `inferred` or `ambiguous`.

**`communities`** is the one whole-graph computation. `store::community_edges`
reads the settled `calls` and `imports` edges in one query, bounded at
`COMMUNITY_EDGES`, and the panel says `Shown N of M`. Label propagation visits
nodes in name order, lets a node keep its own label when it is among the
winners, and runs a fixed number of iterations, so the map does not regroup on
every open; without the second rule, two clusters joined by one edge collapse.

## The retrieval ledger

`retrievals` is append-only and hash-chained: each row stores the previous row's
hash, and its own hash covers its fields plus that link. `store::ledger_break`
re-walks the chain and returns the first row that does not verify, so an edited
or deleted row is detectable. It costs one blake3 of a short string per query.
`semlith ledger --verify` exits non-zero on a break.

**One writer, four surfaces.** `src/ledger.rs` writes the row for stdio, `/mcp`,
the CLI and the portal alike. A row carries the client's name from the MCP
`initialize` handshake (resolved to the documented client name where the app can
be told) and its session. Graph tools record against the bytes of every file a
grep for that name would have made you read. A retrieval that found nothing is
recorded and credited nothing.

**The chain is versioned by the row.** Columns added later (`session`, `tool`,
`stale_hits`, `tokenizer`) would change the hashed fields, so `tool` — NULL on
every row written before 0.15.0 — selects the formula. A store holding both kinds
verifies end to end. `client_version`, `query_id` and `synced_at` sit outside
the chain.

**Tokens are counted with the tokenizer already loaded** — the store model's
`tokenizer.json`, from the same cache under the same pinned digest. Four
characters per token is the fallback for a session that never loaded a model (a
graph-only session, or an airgapped machine with nothing cached). Each row
records which counted it, in `tokenizer`, so the two are never summed together.
`semlith stats` prints tokens not read, over how many of how many retrievals,
with coverage and whether the figure is `measured` or `modelled`; it does not
deduplicate files across a session, so it is an upper bound.

**Recording is on by default**, under rules you can check: rows never leave the
store they were written into (a packet capture proves it); the daemon prints
`ledger: recording (local only; --no-ledger to stop)` or `ledger: off for this
session` on every start; and erasing every row is one `DELETE`. `--no-ledger`
stops a session and `SEMLITH_LEDGER=0` a machine; both are read at write time.
A search whose text holds a live-shaped key is recorded masked.

## Reports and replay

`src/report.rs` is one structure and five renderers: a report is a title and a
list of blocks (a sentence, a row of counted facts, or a named table), and
Markdown, CSV, JSON, print-styled HTML and PDF are readings of it. `semlith
report` and the portal's export therefore produce the same bytes. The PDF is
typeset from the blocks in the base-14 Courier faces, where every glyph is 0.6
em, so a line's width is its length and no font is embedded. `Report::render`
returns a `String` for the four text formats; `Report::render_bytes` serves all
five. A surface that can only print text calls `check_format`; one that can
return bytes calls `check_any_format`.

The savings report never sums its three counterfactual lines (whole files not
read, excerpts read instead, refunds), because they would count one saved read
up to three times. Every figure carries its denominator and its tokenizer tier.

The ledger cannot see whether an answer was enough; the agent's transcript can.
`src/replay.rs` reads it: a whole-file read after a retrieval is a refund, a
grep is a miss, an edit means the answer sufficed. It reads Claude Code's
transcripts only, it is on by default (the Privacy page turns it off), and
nothing it reads leaves the machine.

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

## Considered and left out

- **A daemon reachable from other machines.** The daemon binds `127.0.0.1` only,
  with no flag to change it. The property protected was never "no port" but
  "nothing about semlith reaches the network, and you can check it in a minute";
  `--airgap` makes that falsifiable. See
  [#41](https://github.com/semlith/semlith/issues/41).
- **Embeddings in SQLite.** Every query would scan blobs out of SQLite or
  duplicate them in memory. A purpose-built index is what makes queries fast.
- **A graph library or a graph build step.** See [the code graph](#the-code-graph).
- **Per-file license headers.** Apache-2.0 recommends but does not require them;
  the Rust convention is the `license` field in `Cargo.toml` plus a `LICENSE`
  file, and both are present.
