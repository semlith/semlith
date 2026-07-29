<!--
Generated from canonical UltraShip state. Do not edit directly.
Run `ultraship views` to regenerate.
-->
# Release history

## semlith

| Version | Released | Mode | Delivered |
| --- | --- | --- | --- |
| 0.2.0 | 2026-07-29T05:40:00Z | published | Index a repository on an ordinary 8 GB laptop without watching memory or fearing a second terminal, then find an exact identifier and a plain-English question with the same search. |

### 0.2.0 known limitations

- One writer per store. A second index run is refused rather than queued; there is no wait-for-lock option.
- Indexing remains CPU-bound at roughly 23 chunks/sec, so a 100k-chunk corpus still takes over an hour on first run.
- Query latency grows with corpus size, from under 3 ms at a thousand chunks to 22.7 ms at a hundred thousand. The index scan is linear.
- The default model is English-only.
- The int8 default carries a 1.23 point code MRR deficit against the fp32 build that was never significance-tested; the paired bootstrap was stopped by decision. Hybrid search more than covers the gap in the shipped system.
- All performance figures come from one 4P+4E Apple M1 with 8 GB of RAM. The performance-core thread heuristic in particular is measured on that machine alone, which is why SEMLITH_EMBED_THREADS exists.
- Retrieval was evaluated on a corpus that is 82 percent Rust; the code retrieval gain may not transfer equally to other languages.
- Existing 0.1.0 stores keep BGE-small and are not migrated. Moving one onto the new default means deleting the store and re-indexing.


_Canonical sources: products/<id>/releases/<version>.yaml_
