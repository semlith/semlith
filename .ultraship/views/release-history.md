<!--
Generated from canonical UltraShip state. Do not edit directly.
Run `ultraship views` to regenerate.
-->
# Release history

## semlith

| Version | Released | Mode | Delivered |
| --- | --- | --- | --- |
| 0.3.0 | 2026-07-29T16:02:02Z | published | A developer narrows a search to part of an indexed corpus by path glob, file extension or language — from the CLI and from an agent over MCP — and gets the best matches inside that subset, because the filter is applied before either half of the hybrid search picks its top-k rather than after. |
| 0.2.0 | 2026-07-29T05:40:00Z | published | Index a repository on an ordinary 8 GB laptop without watching memory or fearing a second terminal, then find an exact identifier and a plain-English question with the same search. |

### 0.3.0 known limitations

- Unfiltered search latency was not re-measured on a 100k-chunk store for 0.3.0. The criterion was replaced by structural proof that the unfiltered path is unchanged plus identical results across six A/B runs; see US-SEMLITH-0.3.0-I01. The README's 105k-chunk figures remain 0.2.0's measurement and are not restated as 0.3.0's.

- The Windows path-separator translation in filter::anchor is unexercised. CI runs Ubuntu and macOS only, so the cfg(windows) branch that rewrites `/` to `\` has never executed in a test on any machine, despite Windows being a shipped platform. A Windows user's first glob is the first run of that code.

- Filters are SQLite GLOB: `*` crosses `/`, there is no distinct `**`, no regex, and no way to express "not this path". Matching folds case via SQL lower(), which is ASCII-only, so a non-ASCII path differing only by case will not match.

- `--lang` is a fixed 25-entry extension table and never reads file contents, so an extensionless script is not recognised as its language.

- Filters are not available on `semlith files` or `semlith stats`, so there is no way to preview which files a glob selects other than running a search and reading the reported count.

- Scoped latency and rank behaviour were measured on small fixtures of a few hundred chunks, not on a repository-scale corpus. A filter that selects a very large subset is untested for the crossover where passing a big allowlist costs more than an unfiltered scan.

- This release did not follow the project's `feat/<version>` → develop → main pull-request flow. Work was committed directly to main and `feat/0.3.0` was created afterwards at the release commit, so the branch records what shipped but not a review path.


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
