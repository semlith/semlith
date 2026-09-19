# Performance

Every number semlith publishes, with the command that reproduces it and the
release it was taken for. A figure here is allowed to name a version, because a
measurement without a date is a claim rather than a measurement.

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM:

```sh
cargo test --release --test measure -- --ignored --nocapture
cargo test --release --test shards -- --ignored --nocapture
cargo test --release --test retrieval -- --ignored --nocapture
```

The `--release` is not optional. `tests/measure.rs` and `tests/shards.rs` assert
real thresholds, and a debug build fails them honestly rather than usefully.

The query, watcher and image tables were taken for 0.17.0. The larger-corpus
table is a fixture — building a hundred-thousand-chunk store takes over an hour
of embedding — and is re-taken when the indexing or scan path changes rather
than every release; the recipe is in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Query latency

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
large corpus. The recipe for rebuilding the fixture is in
[CONTRIBUTING.md](../CONTRIBUTING.md).

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
encoder and 244 MB for the text encoder, against 52 MB for the text model. A
corpus with no pictures in it never downloads either.

## What an agent pays

| what | measured |
|---|---|
| `tools/call` over HTTP | **23.4 ms** p50 over 20 calls |
| the same call through the stdio proxy | **20.9 ms** p50, same daemon, same query |
| `tools/list` | **3 995 bytes**, about 999 tokens, for twelve tools with one store open, measured for 0.17.1. `tests/retrieval.rs` asserts it stays under 1 000 tokens, which is the whole reason the figure is watched |
| an MCP server open on one store | 131 MB; on three stores, 132 MB |
| three same-model stores, one search | **1** query embed, +1.7 ms for the second store |

The endpoint costs about what the proxy costs, and the difference is the
client's connection setup rather than the route. Both talk to the same daemon
and run the same search.

## What sharding costs

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



## What a store costs to hold

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

## Choosing a model

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

## The binary, and what a Linux machine needs

| what | measured | when |
|---|---|---|
| the macOS arm64 binary | **116 679 872 bytes** — 111.3 MiB, of which 1.8 MB is the image support | 0.17.2 |
| the same, when forty grammars landed | 116 683 392 bytes | 0.17.0 |
| the Linux glibc floor | **GLIBC_2.34**, with GLIBCXX_3.4.22 | 0.17.1's artifact |

```sh
ls -l target/release/semlith
objdump -T semlith runtime/libonnxruntime.so | grep -o 'GLIBC_[0-9.]*' | sort -V -u | tail -1
```

The glibc figure is taken over the pair that ships, not the binary alone: a
Linux archive holds `libonnxruntime.so` beside `semlith`, what a user runs is
both, and the floor is whichever of the two needs more. `release.yml` measures
it on every tag and fails above GLIBC_2.35, which is Ubuntu 22.04 LTS — the
oldest distribution the project promises to start on.

## Retrieval quality

Measured for 0.22.0 over the 107-question harness in
`tests/fixtures/retrieval/questions.yaml`, against the corpus pinned beside it
at `tests/fixtures/retrieval/corpus` — the 0.21.0 tree at commit `4e8df39`, 146
files and 3 119 835 bytes, asserted by file count and byte total on every run:

```sh
cargo test --release --test retrieval -- --ignored --nocapture
```

The set is split 77 development / 30 sealed. Every ranking decision in this
release was read off the development set; the sealed thirty are scored once, at
the end. Both are here, and 0.21.0 is measured on the same corpus through the
same instrument so the comparison is like for like.

| what | 0.21.0 | 0.22.0 |
|---|---|---|
| hit@1, sealed 30 | 18 (60 %) | **20 (66 %)** |
| hit@3, sealed 30 | 22 (73 %) | **23 (76 %)** |
| hit@8, sealed 30 | 25 (83 %) | **25 (83 %)** |
| hit@1, development 77 | 48 (62 %) | **55 (71 %)** |
| hit@3, development 77 | 56 (72 %) | **63 (81 %)** |
| hit@8, development 77 | 65 (84 %) | **67 (87 %)** |
| wrong yes, on `path` | 0 | **0** — asserted, not reported |
| call-edge resolution | | **66 %** settled, against a 50 % gate |

Each figure is the median of three runs, and each run is its own index of the
corpus. The spread was zero on every figure of every configuration this release
measured, which is issue #88's drift gone: the corpus is pinned and the
embedding runs on one ONNX thread.

**The sealed figures move less than the development ones, and that is the point
of having them.** The development questions are the ones the work was tuned
against. The gap between +7/+7/+2 and +2/+1/+0 is what tuning against a visible
set is worth on this corpus.

0.22.0 was specified against a gate of hit@8 95 %, hit@3 85 %, hit@1 70 %. **It
does not meet it** and the gate moves to the next release rather than being
restated as met. What stands in the way is named rather than guessed: seven
concept questions of the development seventy-seven miss at k=8, and twenty more
are found inside the top eight while sitting outside the top three. The second
is a ranking problem, and the two standard levers for it were both built and
both measured here — a larger embedding model gained two questions at k=8 and
none at hit@3, and a cross-encoder over the fused head changed nothing at all at
any depth on either set. Neither ships.

Two earlier figures on this page and in the README are withdrawn rather than
updated. They were taken over a corpus that moved with every commit; against a
question set in which 47 of 92 spans no longer contained the symbol they named;
and by a harness in which no `path` question could record a hit while still
counting in the denominator, which capped hit@8 at 87 % by construction. A moved
number here is a report, never an adjective — and a number from a broken
instrument is not a report at all.
