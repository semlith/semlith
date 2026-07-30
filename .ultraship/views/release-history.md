<!--
Generated from canonical UltraShip state. Do not edit directly.
Run `ultraship views` to regenerate.
-->
# Release history

## semlith

| Version | Released | Mode | Delivered |
| --- | --- | --- | --- |
| 0.8.0 | 2026-07-30T17:45:00Z | published | The documents a corpus is actually made of. `semlith index` now reads `.docx`, `.pptx`, `.xlsx`, `.odt`, `.odp`, `.ods`, `.ipynb`, `.html` and `.htm` as the text a person opening them would see, through the same `chunk::extract` hook PDF has used since 0.1.0 — no new command, no new flag, no store format change. The comparison is the release. The same directory of eight documents, indexed by the real 0.7.0 binary from crates.io and by this build, asked the same eight questions: 0.8.0 answers all eight with the file that holds the phrase. 0.7.0 answers two, and the other six are not misses but confident wrong answers — it hands back the notebook or the HTML page, the only two files it could read at all, for a phrase sitting inside an archive it skipped as binary. What gets embedded is the content and not the syntax: a notebook went from 1319 characters in the store to 454, an HTML page from 699 to 292, on the same files. Structure that a line number cannot express is named — `# Slide 11`, `## Sheet: Q3 Notes`, `# Cell 2 (code)` — and an HTML hit still points at the line of the file on disk, because every newline in the source survives extraction, including the ones inside the tags that were removed. A file semlith cannot read is a file it walks past, as an unreadable PDF always has been: corrupt, truncated, encrypted, or expanding past the new 32 MiB decompression cap. An archive whose single entry inflates to 64 MiB costs a run nothing — 271 MB peak RSS with it in the corpus, 271 MB without — and a corpus of ordinary Rust pays nothing either, at 18.98 s against 0.7.0's 18.66 s on the same tree. |
| 0.7.0 | 2026-07-30T15:35:00Z | published | A corpus larger than one repository, on the machine the developer already has. A store created by 0.7.0 keeps its vectors as fixed-size shards under `index/`, recorded as `format_version` 2, and three things follow from that one change. Opening such a store reads no vectors at all, so `stats`, `files` and an MCP server waiting to be asked something cost a model and nothing else — measured at 133 MB whether the store holds 700 chunks or 70000, where 0.6.0 went from 137 MB to 180 MB over the same corpora. Searching holds what `SEMLITH_INDEX_MEMORY` allows, 512 MB by default, putting down the coldest shard to stay inside it and saying so. And a long index run makes its work durable every thirty seconds before recording the files it covers, so an interruption costs the last half-minute: killed eight seconds into a 6000-file corpus, 0.6.0 kept nothing and 0.7.0 kept 1952 chunks, searchable with no repair pass and skipped by the run that followed. An index run now says where it is — files of files, chunks per second, an estimate of what remains — and a resumed run says how many files it walked past. Changing one file rewrites the shards it touches rather than the index: 176 KB of a 1436 KB index on a store of fourteen shards. Every store written before 0.7.0 keeps working exactly as it did, on its single `index.tv`, unmigrated, with its `format_version` untouched — proved through real 0.5.0 and 0.6.0 release binaries in both directions. |
| 0.6.0 | 2026-07-30T04:06:55Z | published | A developer wires semlith into whichever agent they use — twelve clients have a stanza in the README, each verified against that client's own documentation and each executed by a test that reads it straight out of the README. The server speaks every MCP revision it advertises: 2026-07-28, 2025-11-25, 2025-06-18 and 2024-11-05, with a recorded session for each, and it no longer answers a handshake with whatever revision it was sent. The stateless 2026-07-28 era is served alongside the handshake, decided per message rather than per connection. The tool surface is five rather than two: search, stats, files, index and forget, with both writers taking the store lock per call and index bounded by a budget that reports what remains and resumes. And docs/compatibility.md says which surfaces are a contract and which are not, backed by a format_version marker in the store. |
| 0.5.0 | 2026-07-30T02:01:13Z | published | A developer points one search at several stores — `semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith` — and gets one ranked list back where every excerpt names the store it came from. An agent gets the same thing from one `semlith mcp` process opened on several stores, in one tool call, with an optional `store` argument to narrow it. `-k` is global, filters reach every store, stores whose embedding models differ are searched together, and writes stay single-store. |
| 0.4.0 | 2026-07-29T18:30:54Z | published | A developer leaves `semlith watch` running over a repository and keeps working: a saved file is re-embedded about a second later, a new file is picked up, a deleted file loses its vectors, and a rename moves the file rather than duplicating it — with no `index` run. An agent already connected to that store over MCP sees the change on its next search, because the store counts index rewrites and a search reloads the vector index when the count has moved. |
| 0.3.0 | 2026-07-29T16:02:02Z | published | A developer narrows a search to part of an indexed corpus by path glob, file extension or language — from the CLI and from an agent over MCP — and gets the best matches inside that subset, because the filter is applied before either half of the hybrid search picks its top-k rather than after. |
| 0.2.0 | 2026-07-29T05:40:00Z | published | Index a repository on an ordinary 8 GB laptop without watching memory or fearing a second terminal, then find an exact identifier and a plain-English question with the same search. |

### 0.8.0 known limitations

- This record was sealed before the crates.io upload. delivery.target_mode is `published` and the remaining steps make it true: the feat/0.8.0 → develop → main merges, the v0.8.0 tag and its GitHub release, then cargo publish from this machine. The record states the authorised target rather than quoting a registry timestamp.
- HTML is now read as text for every .html file, including template files. A repository of Handlebars or Jinja templates loses its markup from the index: a search for `class="btn-primary"` no longer matches, though the expressions inside the markup survive and line numbers stay true. There is no flag to opt out, and this is the change most likely to be discovered by a user rather than by a test.
- A file already in a store is not re-read while its bytes are unchanged, so a corpus of HTML or notebooks indexed before 0.8.0 keeps its old markup and JSON chunks until those files change. `semlith forget <PATH>` converts one file; a corpus means deleting the store and indexing again. The CHANGELOG says so.
- Line numbers for the archive formats and notebooks are positions in the extracted text, not in the file on disk — the same as PDF has always been. Only HTML keeps file-true lines. The markers exist because of this: `# Slide 11` is what makes such a locator usable.
- The markers are text in the chunk, so they are embedded with it and appear in excerpts. Whether they help retrieval or dilute it was not measured; they were chosen for legibility.
- The 8 MiB per-file cap is unchanged, so a deck or document that is large because of its images is skipped for its pictures — measured on a real 24 MB PowerPoint file, which is skipped despite holding little text. The contract left this as an open question and the answer is a deliberate no change, not an oversight.
- Not read: legacy binary .doc, .xls and .ppt, RTF, EPUB, Pages/Numbers/Keynote and mail archives. A password-protected Office document is skipped, not decrypted. Speaker notes, comments, tracked changes, headers and footers are not extracted, and spreadsheet formulas are not evaluated — a cell's cached value is what is indexed.
- Nothing images or infers: no OCR, no scanned PDF, no chart contents. Alt text is taken only where it is already text.
- The committed fixtures were written by python-docx, python-pptx, openpyxl and odfpy — real libraries, not hand-written XML, and they found two real bugs. Files written by Word, PowerPoint and Excel themselves were exercised as health checks off this machine and deliberately not committed, so the suite that runs in CI is the library-written set.
- Notebook outputs are truncated at 2000 characters each and only a stream's text or a result's text/plain is kept. A cell whose interesting output is a widget or an image contributes nothing but its source.
- The XML scan is a scanner, not a parser: it does not resolve namespaces, so a document using unusual prefixes for the Word or ODF elements would come back thinner than it should. Every file tested uses the conventional prefixes, which is what the writing tools emit.
- All numbers here are from one machine, an Apple Silicon laptop. CI runs the suite on ubuntu-latest and macos-latest, but the RSS and timing figures are from the one machine, and RSS moves by a few MB run to run regardless of what is being measured.

### 0.7.0 known limitations

- This record was sealed before the crates.io upload. `delivery.target_mode` is `published` and steps 3 to 7 of the release make it true: the feat/0.7.0 → develop → main merges, the v0.7.0 tag and its GitHub release, then `cargo publish` from this machine. The record states the authorised target rather than quoting a registry timestamp.
- A store created by 0.7.0 cannot be read by 0.6.0, which refuses it naming both format numbers. That is the break, and it is announced in the CHANGELOG and docs/compatibility.md.
- 0.5.0 and earlier predate `format_version` and have nothing to check, so such a binary reads a 0.7.0 store as an empty corpus rather than refusing it — measured, exit 0 with 'no matches (store has 0 chunks)'. Nothing in 0.7.0 can fix a check an older binary does not make.
- There is no migration for an existing store. Re-indexing is the only way onto the sharded layout, and for a large corpus that is hours. The vectors in an index.tv are quantized and cannot be split back out.
- A modified file moves two shards, not one: the shard losing its old vector and the newest shard taking the new one. So a one-file save is bounded by two shards rather than by a fraction of the store, and below roughly 131000 chunks at the shipped shard size that is still the whole index — which is exactly what the 70000-chunk measurement shows.
- The budget bounds the vectors held resident, not the process's RSS. A store past its budget was measured settling at 110 to 126 MB across 200 queries, but that is the allocator returning what it frees, not a guarantee this release makes.
- Everything was measured to 70000 chunks, not to millions. The curve is measured; a larger corpus is arithmetic from it — roughly 640 bytes of resident memory per vector while searching, from the measured slope.
- The scale corpus is one short chunk per file, shaped for vector count rather than for realistic prose: what is being measured is what a vector costs to hold, and full-length chunks embed about five times slower to reach the same count.
- The recall comparison is of the fused ranking a user sees, which includes a keyword half identical between the two stores by construction. The dense half was not isolated, so the number is an upper bound on how visible a per-shard calibration difference would be.
- Shard size (65536 vectors) and the default budget (512 MB) come from arithmetic on what a vector costs, not from a sweep of alternatives. The contract's open questions about both are answered by a defensible choice rather than by measurement, and the environment override exists so a machine that disagrees can say so.
- Two tests that shipped before this release were edited. src/store.rs's a_newer_format_is_refused_naming_both_numbers now says FORMAT_VERSION + 1 instead of the literal 2, which is what the test always meant; and tests/measure.rs learned to size an index in either layout. Callback signatures were updated mechanically where index_paths gained its progress argument. No assertion was weakened.
- Checkpointing, the memory budget and one-shard saves are properties of the sharded layout, so a store created before 0.7.0 gets none of them — an interrupted index run on one still loses the run.
- The memory and latency figures come from one machine, an Apple Silicon laptop. CI runs the suite on ubuntu-latest and macos-latest, but the numbers in the README and in this record are from the one machine.

### 0.6.0 known limitations

- The 2026-07-28 path has never met a client that speaks it. That revision is two days old and its SDKs are in beta, so server/discover, the per-request _meta version, resultType and the cacheable list fields are all proven against semlith's own reading of the specification and nothing else. The handshake path, which every client shipping today uses, is proven against real sessions.

- 2025-03-26 is not advertised, because it is the one revision that required JSON-RPC batching. A client pinned to it is answered 2025-11-25 and has to accept that or disconnect. No known client is pinned there.

- The twelve client stanzas are verified against documentation and executed as command lines; none of the twelve clients can be installed in CI, so none of them has actually loaded semlith in this release's testing. Two vendors document their own format ambiguously: Cline gives two different config paths across two pages, and Cursor's reference table marks `"type": "stdio"` required while its own example omits it. semlith includes it.

- semlith_index blocks the server for the length of its call. The message loop handles one request at a time, so an index running to its 45-second budget makes the server unresponsive to a search from the same client for that time. The budget bounds it; it does not overlap it.

- semlith_index will embed whatever it is pointed at. .gitignore is honoured, which covers the common case of a `.env` beside the code and not every case, and the budget bounds a runaway rather than preventing one. An agent now has a tool that writes to the store, which 0.5.0 did not give it.

- The budget reports paths remaining, not bytes or chunks. "40 remaining" says nothing about how long the next call takes, and a corpus of large files can take several calls where the count suggests one.

- format_version is a single number with no minimum-reader field, so a future format that an older binary could read but not write cannot be expressed. That distinction would need a second key and a new format version to introduce it.

- `semlith forget` taking the write lock is a behaviour change for the CLI too, not only for the tool. A `forget` that used to succeed while `semlith watch` was running now exits non-zero naming the watcher. Deliberate — the old behaviour could leave the index and the database disagreeing — but a 0.5.0 script can see it.

- The tool list roughly doubled: 2220 bytes for two tools to 4790 for five, about 555 to 1197 estimated tokens, paid by every agent in every session whether or not a tool is called. That is the cost of the wider surface and it is spent on the context this product exists to save.

- semlith_files defaults to 200 paths. A store holding a large repository returns the cap and a count, so an agent that does not narrow with path/ext/lang sees a fraction of what is indexed.

- Windows remains unexercised by CI, as recorded for 0.5.0's path-list splitting, 0.4.0's watcher and 0.3.0's path handling. Both new write tools take the same advisory lock, whose behaviour there is untested.

- lib.rs was deliberately not restructured. The compatibility page names which modules are supported, but chunk, lock and store remain public exactly as they were, so the documented surface and the compiled surface are not the same shape.


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
