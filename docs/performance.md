# Performance

Every number semlith publishes, with the command that reproduces it and the
release it was measured for. Use it to size a machine or to check a claim.

Unless a section says otherwise, the reference machine is an M1 Air: 4
performance and 4 efficiency cores, 8 GB of RAM, fanless. The harnesses:

```sh
cargo test --release --test measure -- --ignored --nocapture
cargo test --release --test shards -- --ignored --nocapture
cargo test --release --test retrieval -- --ignored --nocapture
```

`--release` is required: `tests/measure.rs` and `tests/shards.rs` assert real
thresholds, and a debug build fails them.

The query, watcher and image tables were measured for 0.17.0. The larger-corpus
table uses a fixture store that takes over an hour to build and is re-measured
only when the indexing or scan path changes; the recipe is in
[CONTRIBUTING.md](../CONTRIBUTING.md).

Indexing figures before [the service section](#the-service-and-the-cpu-and-gpu-together)
were taken in a terminal, at the priority the terminal gave. Most users index
through the login service; that section has its figures.

## Query latency

Warm, server-side, as the daemon reports it. A query is embedded once per vector
space the store holds, and at these sizes that embedding is most of the cost.

| store | p50 | what is in it |
|---|---|---|
| text | **16.0 ms** | 2 220 chunks over 85 files |
| images | **9.1 ms** | 120 images, no text |
| a fleet of both kinds | **25.0 ms** | the text store above, beside one holding images |

The two halves add rather than interfere. A store with no images skips the
image space before a model is loaded.

Over three larger corpora of mixed Rust, Markdown and TypeScript:

| store | warm query, p50 | p95 | indexing | peak RSS |
|---|---|---|---|---|
| 1.2k chunks | **2.7 ms** | 3.5 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | **5.4 ms** | 11.0 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | **22.7 ms** | 67.4 ms | 23.5 chunks/sec | 595 MB |

**Peak memory does not grow with the corpus**: plan for about 600 MB whatever
you index. **Query latency does grow**, because the index scan is linear: a few
milliseconds for a repository, a few tens for a very large corpus.

## A large multi-repository store

Measured for 0.33.1 on 2026-09-30: 70 public repositories pinned to release
tags, 879 439 chunks from 66 333 files, 521.7 MB of indexed text. Every search
went through the login service: a stdlib Python harness called the daemon's
`/mcp` endpoint, dropped the first three calls, and ran three sessions. Each
range is the lowest and highest of the three.

| search | p50 | p95 |
|---|---|---|
| concept | **257–363 ms** | 500–920 ms, max 1.6 s |
| identifier | **96–111 ms** | 294–346 ms |
| scoped to one repository | **574–597 ms** | 803–910 ms |
| concept, `excerpt` format | **335–372 ms** | 532–545 ms |

Where a search's time went, from temporary stage timers in a measurement-only
build: one session through the daemon, 72 warm searches.

| stage | p50 | p95 | max |
|---|---|---|---|
| query embed | **3.1 ms** | 3.8 ms | 4.1 ms |
| vector scan, 879 439 vectors in 14 shards | **5.1 ms** | 6.1 ms | 15.1 ms |
| full-precision rescoring | **11.2 ms** | 23.0 ms | 35.6 ms |
| keyword list (FTS5) | **92.0 ms** | 335.7 ms | 414.4 ms |
| image list | **6.2 ms** | 7.6 ms | 8.0 ms |
| graph list | **112.3 ms** | 438.3 ms | 2 120.1 ms |
| everything inside the store | **328.3 ms** | 748.5 ms | 2 267.4 ms |

The linear scan is not the cost at this size, so an approximate-nearest-neighbour
index would save only a few milliseconds; the keyword and graph lists are the
cost. (Search stopped building the graph list in 0.36.0; see
[architecture.md](architecture.md#ranking).) The first search after a start paid
3.4 s once in the image list to load CLIP, because the corpus holds two PNG
images. The daemon's RSS was 525–727 MB.

**A search scoped to one repository is about twice as slow as an unscoped one**,
because the path filter did not reach the graph's edge lookups.

The first index of this corpus took 70.7 min, a mean of 207 chunks/s on the
Neural Engine lane (0.32.0 daemon, 2026-09-29, mostly on battery, low power mode
off).

## Keeping it current

| what | measured |
|---|---|
| edit on disk to searchable | **1.24 s**, watcher running |
| ten saves in six seconds | **1** index write of 638 KB |
| re-indexing 120 unchanged files | **0.01 s** — content hashes match, no model is loaded |
| idle daemon | **0.01 s** of CPU over 60 s, 10 MB resident |
| 100 edits | 37 MB resident at start, 59 MB after |

## Images

| what | measured |
|---|---|
| indexing 120 images | **10.0 s**, including loading the vision encoder |
| warm image query | **9.1 ms** p50 over 15 queries |
| the image index | 600 KB for 120 images, beside the text index |
| resident, after an image query | 191 MB |

CLIP is fetched only when a store first indexes an image: 335 MB for the vision
encoder and 244 MB for the text encoder, against 52 MB for the text model.

## What an agent pays

| what | measured |
|---|---|
| `tools/call` over HTTP | **23.4 ms** p50 over 20 calls |
| the same call through the stdio proxy | **20.9 ms** p50, same daemon, same query |
| `tools/list` | **2 633 bytes** for the eight listed tools (0.36.0); `tests/retrieval.rs` asserts it stays under 675 tokens |
| an MCP server open on one store | 131 MB; on three stores, 132 MB |
| three same-model stores, one search | **1** query embed, +1.7 ms for the second store |

The HTTP endpoint costs about what the proxy costs; the difference is the
client's connection setup. Both reach the same daemon and run the same search.

## What sharding costs

A store split into 16 shards returns the same top ten as one shard **about 90 %**
of the time, over twelve questions about meaning rather than identifiers. The
shards keep peak memory flat; that is the price.

The figure moves between 0.88 and 0.98 from run to run. ONNX Runtime reduces
across its threads in whatever order they finish, so a question does not embed
to exactly the same vector twice, and a vector near a tie changes which of two
near-equal chunks comes back. Pin `SEMLITH_EMBED_THREADS` when you need a
reproducible index.

## What a store costs to hold

An idle MCP server holds a model and little else: a hundredfold more corpus
costs an open store **0.8 MB**, because vectors are read when a question is
asked. Searching holds the vectors it searches: 70 000 chunks is 43 MB, well
inside the 512 MB default. Past that budget the store keeps what it can and reads
the rest back per query, so a corpus larger than memory stays searchable, at a
cost: the same 70 000-chunk store squeezed into an 8 MB budget answered in
364 ms instead of 53 ms.

Changing one file rewrites the shards it touches, not the whole index. On a
store of 14 shards, re-indexing one changed file wrote 176 KB of a 1 436 KB
index: two shards, because the shard losing the old vector and the newest shard
taking the new one both change. The saving appears once a store is more than
two shards, around 131 000 chunks at the default shard size.

An index run killed eight seconds into a 6 000-file corpus kept **1 952**
chunks, not zero. Vectors are made durable before their files are marked
indexed, so a killed run resumes and never leaves a file marked indexed that
cannot be answered for.

## Choosing a model

Indexing is the slow half, and its cost is the embedding model, not the index.
For a large corpus where throughput matters more than some retrieval quality,
`--model AllMiniLML6V2` is about 1.8x faster (6 transformer layers instead
of 12).

Test quantization rather than assume it: the default model's int8 build is both
smaller and faster than its fp32 build on ARM, while BGE's quantized variants
measured no faster than fp32 on the same machine.

Thread count is chosen per CPU, not left to ONNX Runtime; the measurement and
the rule are in [architecture.md](architecture.md#choosing-the-thread-count).
Override it with `SEMLITH_EMBED_THREADS`.

## The service, and the CPU and GPU together

Measured for 0.28.0 with nothing else running, after `df -h`. Each figure is the
median of three runs over one pinned corpus whose hash is in the release record.

**Priority.** The daemon stays in background state while nothing embeds and
returns to normal priority when an embed pass starts. Each switch took under
100 µs, read from the daemon's log. It returns to background 300 ms after the
last embed ends. `ps -o pri` on the service read 4 while idle, 20 while
embedding, and 4 again 0.35 s after the run ended.

| path | chunks/s |
|---|---|
| `semlith index` in a terminal | 27.6 |
| a portal run through the launchd service, CPU lane alone | 15.1, against 24.7 for the CLI in the same rounds (61 %) |
| the same service before 0.28.0 (`ProcessType Background`) | 3.3, against 28 in a terminal, 2026-09-23 |

Three interleaved rounds on 2026-09-24, median, over the `src/` of v0.27.0
(3 746 chunks). The service runs at 61 % of the same binary in a terminal. The
gap belongs to launchd: `semlith start` from a terminal ran at 89 % of the CLI,
and in a launchd agent the embedding threads sit at scheduler priority 20
against 31 from a terminal. Neither a user-initiated QoS request nor `ProcessType
Interactive` closed it. Single runs on the fanless Air moved between 12 and 27
chunks/s over a day of load, so only interleaved pairs are quoted.

**Length-sorted batching.** The index pass sorts a window of up to 64 chunks by
length before embedding, so short chunks are not padded to a long one's length.
A standalone script over 512 real chunks measured 19.5 against 29.1 chunks/s.
In the binary the gain is smaller, because the unsorted path already embedded
eight neighbouring chunks of one file at a time; against unsorted batches of 32,
sorting gives 1.31×. Windows of 128 to 2 048 chunks were within run-to-run
spread of 64.

| CPU lane alone, int8 | chunks/s |
|---|---|
| unsorted, file order | 23.5 |
| sorted, window of 64 | 27.6 (1.17×; a second interleaved round gave 23.4 against 20.6, 1.14×) |

**Lanes.** A GPU lane is a worker process running the fp16 export; the CPU lane
runs int8 in the daemon. Both take batches from one sorted queue, so the faster
device takes the larger share. A portal run through the service with default
settings (CPU and WebGPU on); per-lane rates as the run card showed them:

| lanes | chunks/s | per lane |
|---|---|---|
| CPU alone, sorted | 15.1 | |
| WebGPU on Metal alone, batch 16 | 50.6, measured on its own on 2026-09-23 | |
| CPU and WebGPU (the default) | 36.8, 1.52× the unsorted CPU path (24.1) | GPU 22.4/s · CPU 11.6/s |

**Agreement.** `semlith doctor --gpu` embeds 32 fixed chunks on every lane and
compares them with CPU fp32 vectors in `tests/fixtures/gpu/`. WebGPU on Metal
scored cosine 1.0000 on all 32; the CPU int8 lane scored 0.9851. An fp16 lane
below 0.999 on any chunk is refused before its first batch. int8 and fp16
vectors of the same text agree at cosine 0.987, so a store embedded by both
lanes was compared with an all-int8 store before hybrid became the default. On
the sealed split of 30 questions, CPU lane alone, median of three:

| store | hit@1 | hit@3 | hit@8 |
|---|---|---|---|
| all int8 | 24/30 | 27/30 | 29/30 |
| all fp16 | 25/30 | 27/30 | 28/30 |
| half and half, int8 queries | 25/30 | 27/30 | 28/30 |
| half and half, fp16 queries | 25/30 | 26/30 | 28/30 |

The mix is within one question of all-int8 at every k, so a store may hold both
and queries stay int8. All 11 identifier questions stay in the top three in
every arrangement.

**Memory.** ONNX Runtime's arenas never shrink. In 0.27.0 each writer kept its
peak until exit: 5 392 MB across seven stores, read with `footprint` on
2026-09-23. A writer now releases its session after 60 s without embedding, a
GPU worker exits after 60 idle seconds, and all readers share one query session.

| daemon, seven stores open, GPU lane on | footprint |
|---|---|
| idle | 452 MB |
| during a run | 888 MB |
| 60 s after the last run ends | 465 MB, no writer session loaded |
| 0.27.0, after indexing | 5 392 MB |

`semlith index` in a terminal uses the lanes as the daemon does;
`tests/retrieval.rs` runs on the CPU alone unless `SEMLITH_ACCEL` is set. A
single lane is deterministic, but which lane embeds a chunk depends on timing,
so two hybrid runs give the same chunks but not bit-identical vectors.

## The Neural Engine and the accelerator lanes

Measured for 0.32.0 on AC, in a scratch `SEMLITH_HOME` with the vector cache
off, over the 0.30.1 gate corpus (nine repositories, 20 146 files, 120 000
chunks). The rate is chunks over wall time per slice; the figure is the tenth
percentile over slices two to fifteen, median of three runs.

| run | p10 chunks/s | gate |
|---|---|---|
| the daemon, Neural Engine lane (runs: 229.2, 229.8, 233.3) | **229.8**, median 243 | ≥ 200, met |
| the daemon, Neural Engine off: CPU and the GPU through Core ML | **62.7** (was 16.0 before the batching fix) | ≥ 80, **not met** (#164) |
| the same, the GPU lane alone | 58.2, the lane at 74.4 | — |
| `semlith index` in a terminal against the daemon, one repository, three each | 240.7 against 240.6 | within 10 %, met |

The Core ML GPU model runs about 72 chunks/s alone at batch 8 or 16, and the CPU
lane about 30; together they contend for the four performance cores. A GPU model
converted at six buckets instead of three measured 67.2 and is not shipped.

| other gate | measured | gate |
|---|---|---|
| a cold store's first keyword / graph answer, mid-run semantic p50 | 0.66 s / 0.59 s / 0.91 s, pending share reported | < 2 s |
| re-index after a scripted edit series, cache on against off | 7.62x less embedding time | ≥ 5x |
| a second worktree of the same repository | 0 % of its chunks re-embedded | < 10 % |
| retrieval, 77 development questions, hit@1 / @3 / @8 | CPU 58 / 61 / 67, Neural Engine 58 / 61 / 69, half each 57 / 62 / 67; the 0.31.0 release suite 57 / 61 / 68 | within one question |
| the Neural Engine lane ready after a start: cold, cached, after a rebuild | 39 s (all six buckets 2.5 min later), 2.1 s, 6.0 s | a second process < 2 s to load the models: 0.8 s a bucket |
| the vector cache on disk, and a lookup | 1 703 bytes a vector (a full 1 024 MB cap is about 1.1 GB), 8.6 µs | recorded |
| peak RSS, daemon and its workers, Neural Engine run | 686 MB | recorded |

macOS empties the Neural Engine's compile cache when the disk runs low: with
13 GB free all six buckets were gone within ten minutes; with 32 GB free none
were.

## The binary, and what a Linux machine needs

| what | measured | when |
|---|---|---|
| the macOS arm64 binary | **123 110 976 bytes** — 117.4 MiB, 996 KB more than 0.31.0's for the Core ML bridge; the packs are downloaded, never linked in | 0.32.0 |
| the same | 116 679 872 bytes — 111.3 MiB, of which 1.8 MB is the image support | 0.17.2 |
| the same, when forty grammars landed | 116 683 392 bytes | 0.17.0 |
| the Linux glibc floor | **GLIBC_2.34**, with GLIBCXX_3.4.22 | 0.17.1's artifact |

```sh
ls -l target/release/semlith
objdump -T semlith runtime/libonnxruntime.so | grep -o 'GLIBC_[0-9.]*' | sort -V -u | tail -1
```

The glibc floor is taken over the pair that ships: a Linux archive holds
`libonnxruntime.so` beside `semlith`, and the floor is whichever needs more.
`release.yml` measures it on every tag and fails above GLIBC_2.35 (Ubuntu 22.04
LTS), the oldest distribution the project promises to start on.

## Retrieval quality

Measured for 0.23.0 over the 107-question harness in
`tests/fixtures/retrieval/questions.yaml`, against the corpus pinned at
`tests/fixtures/retrieval/corpus` (the 0.21.0 tree at commit `4e8df39`, 146
files, 3 119 835 bytes, asserted by file count and byte total on every run):

```sh
cargo test --release --test retrieval -- --ignored --nocapture
```

The set is split 77 development / 30 sealed by a recorded seed, and the sealed
thirty are scored once, at the end. The split was redrawn for 0.23.0 because
the 0.22.0 sealed set stopped being held out on 2026-09-19, when eight
questions' spans were completed after it had been scored; the work has now seen
all 107 questions, and `split.yaml` says so. 0.22.0 is measured on the same
corpus, instrument and split.

| what | 0.22.0 | 0.23.0 |
|---|---|---|
| hit@1, sealed 30 | 22 (73 %) | **22 (73 %)** |
| hit@3, sealed 30 | 24 (80 %) | **24 (80 %)** |
| hit@8, sealed 30 | 27 (90 %) | **25 (83 %)** |
| identifiers, sealed 30 | | **12 of 12** in the top three |
| hit@1, development 77 | | **54 (70 %)** |
| hit@3, development 77 | | **62 (80 %)** |
| hit@8, development 77 | | **65 (84 %)** |
| wrong yes, on `path` | 0 | **0** — asserted, not reported |
| call-edge resolution | | **62 %** settled |

Each figure is the median of three runs, each its own index of the corpus; the
spread was zero on every figure (issue #88's drift is gone: the corpus is pinned
and embedding runs on one ONNX thread).

**hit@8 on the sealed thirty fell by two, and the cause is full-precision
rescoring.** The store keeps an `exact.f32` sidecar, and the query path reorders
candidates by the true vectors rather than their 4-bit codes. That is more
accurate but costs recall at k=8, because reciprocal-rank fusion weighs a
candidate by its rank: a chunk the codes placed third can fall far enough under
exact cosine to leave the top eight. It was found by elimination: the fastembed
bump (0.22.0 scores 27 with fastembed 6.1.0 too), the constants extraction, the
candidate-pool depth and a graph-seed interaction were each ruled out, and the
corpus indexed to 4 267 chunks in every run, so chunking was never the cause.

**The sealed figures move less than the development ones**: +2/+1/+0 against
+7/+7/+2. That gap is what tuning against a visible set is worth on this corpus.

Of the 77 development questions, seven concept questions miss at k=8, and twenty
more are inside the top eight but outside the top three — a ranking problem. A
larger embedding model gained two questions at k=8 and none at hit@3; a
cross-encoder over the fused head changed nothing at either depth in this
release. Neither shipped as a default; the cross-encoder later shipped opt-in
(see [models.md](models.md#rescoring-jina-reranker-v1-turbo-en)).

Earlier retrieval figures are withdrawn. They were taken over a corpus that
moved with every commit, against a question set in which 47 of 92 spans no
longer contained the symbol they named, and by a harness in which no `path`
question could record a hit while still counting in the denominator, which
capped hit@8 at 87 %.
