<!--
Generated from canonical UltraShip state. Do not edit directly.
Run `ultraship views` to regenerate.
-->
# Release history

## semlith

| Version | Released | Mode | Delivered |
| --- | --- | --- | --- |
| 0.5.0 | 2026-07-30T02:01:13Z | published | A developer points one search at several stores — `semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith` — and gets one ranked list back where every excerpt names the store it came from. An agent gets the same thing from one `semlith mcp` process opened on several stores, in one tool call, with an optional `store` argument to narrow it. `-k` is global, filters reach every store, stores whose embedding models differ are searched together, and writes stay single-store. |
| 0.4.0 | 2026-07-29T18:30:54Z | published | A developer leaves `semlith watch` running over a repository and keeps working: a saved file is re-embedded about a second later, a new file is picked up, a deleted file loses its vectors, and a rename moves the file rather than duplicating it — with no `index` run. An agent already connected to that store over MCP sees the change on its next search, because the store counts index rewrites and a search reloads the vector index when the count has moved. |
| 0.3.0 | 2026-07-29T16:02:02Z | published | A developer narrows a search to part of an indexed corpus by path glob, file extension or language — from the CLI and from an agent over MCP — and gets the best matches inside that subset, because the filter is applied before either half of the hybrid search picks its top-k rather than after. |
| 0.2.0 | 2026-07-29T05:40:00Z | published | Index a repository on an ordinary 8 GB laptop without watching memory or fearing a second terminal, then find an exact identifier and a plain-English question with the same search. |

### 0.5.0 known limitations

- The merge is not a joint ranking. Each store ranks its own chunks and the merge compares fused rank scores across them, so a store with nothing to say still offers its best hit — it is outranked rather than excluded. Ties are decided by similarity to the query vector, which across two models compares numbers from two vector spaces; that is approximate, and it decides only between hits the rank evidence has already called equal.

- Refusing a store path that is not already a store is a behaviour change for a single-store user too, not only for a fleet. `semlith search --store ./nope` used to create an empty store and report no matches; it now exits non-zero. Deliberate — the old behaviour is undiagnosable in a multi-store query — but it is a change 0.4.0 scripts can see.

- `SEMLITH_STORE` is split on the platform's path separator, so a store path containing `:` on Unix or `;` on Windows cannot be passed that way and has to be given as a flag. Nothing detects that case and warns.

- Store labels come from the directory holding the store. Two stores whose parent directories share a name fall back to their full paths, which is correct and long — an agent then sees a path where it expected a word.

- The latency and memory numbers are three stores of 300 files on one M1. The per-store cost of a hundred-thousand-chunk store was not measured, and neither was a fleet larger than three. Each store holds its own SQLite connection and its own loaded index, so memory grows with store count even though the model does not.

- Windows remains unexercised by CI, as recorded for 0.4.0's watcher and 0.3.0's path handling. This release adds path-list splitting and canonical-path deduplication, both of which are places it could be wrong there.

- A fleet reader picks up another process's writes on search, per store, on the counter 0.4.0 added. `semlith stats` over a long-lived library handle still reports what each store held when it was opened.


### 0.4.0 known limitations

- The Windows `notify` backend has never run. CI is Ubuntu and macOS only, so the platform that gets an archive built for it is the one platform whose filesystem-event path is unexercised — the same gap 0.3.0 recorded for its Windows path-separator branch, now covering a whole feature.

- Ctrl-C is not clean on Windows. The signal handler is Unix-only, because Windows console handlers are a different mechanism, so an interrupted watcher there can leave a file to be re-indexed on the next run and can strand an index.tv.tmp until the next indexing pass clears it.

- A watcher only sees what happens while it runs. Changes made while it was down are caught by its next startup pass, not reconstructed, and there is no reconcile for events lost to a backend queue overflow — a `git checkout` of a very large branch during a busy moment is the realistic case.

- `watch` holds the store's write lock for its whole life, so `semlith index` against that store is refused for as long as it runs. Correct, and a behaviour change for anyone who scripted `index`.

- Every batch that changes anything rewrites the whole index.tv. Measured at 202 KB for a 1000-file corpus, which is nothing; on a 100k-chunk store it is a multi-megabyte write per edit, and that crossover was not measured. The same untested-at-scale caveat 0.3.0 recorded for filtering applies here.

- Each batch walks the watched tree to decide which event paths the indexer would have accepted. That keeps one definition of "which files count" instead of two, at the cost of a directory walk per batch, unmeasured on a tree with hundreds of thousands of entries.

- Network filesystems are unsupported: they do not deliver reliable events. Nothing detects that a root is on one, so the failure mode is a watcher that runs and silently sees nothing.

- A reader picks up new vectors only when it searches. `semlith stats` over a long-lived library handle still reports the vector count from when the process opened the store.


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
