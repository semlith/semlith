# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0] - 2026-09-13

The portal becomes the product's own design, agents get an endpoint they can
keep in a config file, and a query can find a picture.

### Added

- **MCP over HTTP, at `http://127.0.0.1:7365/mcp`.** A client that would rather
  hold a URL than spawn a process now can. The same `mcp::answer` the stdio
  server runs answers it, behind the same loopback bind, `Host` allow-list and
  absent CORS the portal enforces, and `semlith mcp` over stdio keeps working
  exactly as it did.
- **A persisted agent key, in `~/.semlith/agent.key`.** 32 bytes from the OS
  random source, written once with mode 0600 and never reminted implicitly, so
  a stanza carrying it is written once and keeps working across daemon
  restarts, upgrades and portal token rotations. It authenticates `/mcp` and
  nothing else: every `/api/*` route still requires the per-run session token,
  so a credential sitting in a client's configuration file cannot reach the
  rotate, adopt or upgrade routes. Pressing Rotate on the Privacy page no longer
  disconnects a connected agent, because the two credentials have separate
  lifetimes and separate reach.
- **`semlith key show` and `semlith key rotate`.** Show prints the key and the
  stanza around it. Rotate mints a new key, tells a running daemon so the
  endpoint takes it up, keeps the previous key valid until that daemon exits so
  a session already open finishes, re-registers Claude Code through its own CLI
  and names every other client that needs the new stanza. `--now` drops the
  previous key immediately.
- **A rotation carries the key into the files that hold it.** `semlith key
  rotate` and the portal's Rotate key button rewrite every client
  configuration file on this machine that already carried the old key — each
  client's documented path, plus the project-scoped files beside wherever the
  daemon was started — and both then list what they changed. A file is touched
  only when the exact old key appears in it, it is replaced through a
  neighbouring temporary file rather than in place, and no file is ever
  created. Rotating used to leave every configured client authenticating with
  a key the daemon had stopped accepting.
- **Bulk forget, and deleting a store.** The Files page carries a checkbox per
  row and forgets the selection in one call, reporting how many files and
  chunks went and naming anything selected that was never indexed;
  `/api/forget` takes `paths` as well as `path`, and one path answers exactly
  as it did. `semlith drop <store>` and the Stores page's Delete remove a
  store's vectors, chunks, graph, ledger and registry entry. The corpus is not
  touched. A store held by a running daemon is closed through it: the watcher
  stops and releases its lock, the readers are dropped, and only then are the
  files removed — so the daemon loses a store rather than its life.
- **An index run says what it is doing.** The stream carries a line before the
  writer reaches the job — it is one thread, and it finishes what the watcher
  is doing first — and then a line per file with what happened to it:
  indexing, unchanged, skipped or removed. Only the files being embedded used
  to be reported, so a re-index of an unchanged corpus said nothing at all
  between "started" and "done", which reads as a hang. The Index view shows
  the outcome, the running chunk count and the rate beside each path.
- **A request no longer waits for the watcher.** The catch-up pass a store
  runs when the daemon starts steps aside the moment anything is queued, and
  finishes itself when the queue is empty again — so the first index from the
  portal on a cold store begins in about 200ms rather than after the whole
  tree. Shutdown interrupts it too.
- **Pause and stop an index run.** Start becomes Pause while a run is on, and
  Stop asks first: stopping undoes everything the run embedded, so the store is
  exactly as it was before it started and the next attempt begins at 0%. A
  half-indexed corpus is worse than none, because nothing in the store says
  which half it is. `POST /api/index/control` takes `pause`, `resume` or
  `stop`; the run is asked at each file boundary, never inside a file. A stop
  asked for while the job is still queued is answered from the queue itself, in
  about a millisecond, because nothing of it has run.
- **An endpoint switch.** `semlith start --no-mcp-http` starts with `/mcp`
  closed, and the Agents page starts and stops it while the daemon runs.
  Closing it drops the route, not the daemon: the stores stay open, the watcher
  keeps running and the portal is unaffected.
- **Image search.** `.png`, `.jpg`, `.jpeg`, `.webp` and `.gif` files are
  embedded locally with CLIP ViT-B/32 into a second vector space beside the text
  index, and a text query is embedded with the matching text encoder, so a
  sentence such as "the architecture diagram with the queue" finds the picture.
  An image hit carries its path and pixel size where a chunk carries a line
  range, in the CLI, in `--json`, over MCP and in the portal, which previews it
  inline. It is not OCR: an image is matched by what it depicts, and a dense
  text screenshot ranks poorly. The two model files are fetched on the first
  image a store indexes rather than at start, go to the same cache as the text
  model, and `--airgap` refuses them by name — a store that never holds an image
  never downloads them.
- **Sorting and pagination on every table the portal renders.** One component
  behind all of them, so a table cannot gain sorting on one page and not
  another. The Files table sorts and pages on the server, so ordering a column
  orders the whole store rather than the rows that happened to be loaded, and
  the silent truncation at 500 files is gone.
- **An `indexed` column on the Files page**, and a header that states files,
  formats and stores from the store's own queries rather than from the rows on
  screen. The Stores page gains the same kind of measured totals: lines across
  every indexed file, how many formats they span and how many readers are in
  use.

### Changed

- **The portal is rebuilt against its design.** The Graph page draws symbols as
  labelled rounded boxes on a full-bleed canvas with arrowheads, solid edges for
  extracted and dashed for inferred, the selection in accent with its
  neighbours lifted, a floating legend, edge-kind filters and a hover card
  naming a symbol's kind, file, store and caller and callee counts. Stores,
  Index, Agents, Ledger, Privacy and About are rebuilt to match. Type, spacing,
  line height, motion, hover and focus all come from one token block rather
  than from each component, and every page has one scrolling region, so a
  header no longer scrolls away with the content under it.
- **The product is written "Semlith" in prose**, and stays lowercase wherever it
  is an identifier: every command, every MCP tool name, `~/.semlith`, the crate
  and the binary. A command inside a sentence is set in mono, which is what
  makes the difference read as a rule rather than as a typo.
- **The Linux prebuilt binaries are built on 22.04 runners**, so they need
  glibc 2.35 rather than 2.39 and run on Debian 12, Ubuntu 22.04 LTS, RHEL and
  Rocky 9 and Amazon Linux 2023. The release build now asserts the floor, so it
  cannot rise again without failing. Closes [#57](https://github.com/semlith/semlith/issues/57).
- **`initialize`, `tools/list` and `ping` answer with no store open.** An agent
  connecting to a fresh install is told which tools exist rather than that the
  daemon has nothing to serve.

### Removed

- **`semlith impact`, the `semlith_impact` MCP tool and `GET /api/impact`.**
  Reverse reachability leaves the free product whole rather than in pieces — the
  Impact page, the Graph rail's blast-radius action, the Search page's link to
  it, `Fleet::impact_in` and `graph::impact` go with them, and the advertised
  tool count drops from ten to nine. It returns in 0.14.0 as a paid surface.

  **This is a break in the agent-facing surface.** An agent with
  `semlith_impact` in a saved prompt or a committed `.mcp.json` breaks on
  upgrade, and there is no shim. `semlith symbol`, `semlith neighbors`,
  `semlith path` and their three MCP tools are unchanged.

### Fixed

- **A tooltip inside a table was unreadable.** It was drawn as a `::after` on
  the element it described, and `.table-wrap` carries `overflow: auto`, so a
  scroll container clipped it — the Files page's path tip, which is the reason
  the feature exists, was the one that could not be read. There is now one
  tooltip for the whole portal, in a fixed element outside every scroll
  container, which flips rather than overflowing the viewport and answers
  keyboard focus as well as the pointer.
- **The Index page painted an empty bordered box.** `.note` was declared twice
  — inline in one place and as a padded card in another — and the later
  declaration won for every user of the class. `.pill` had the same defect. Both
  are now declared once.
- **`--airgap` could have fetched a model.** The check asked whether the model
  cache held anything rather than whether *that* model was in it, so a machine
  with the text model pre-seeded would have downloaded CLIP on the first image
  it indexed. It now asks about the model it is being asked to load.
- **A route that no longer exists answers 404** rather than 405. A removed
  route reading as "wrong method" is an invitation to go looking for the right
  one.
- **The install script's progress bar, properly this time.** 0.12.0 claimed to
  have fixed this by resolving the release URL's redirect first. That was the
  wrong diagnosis and the fix was close to a no-op: measured against the real
  16 MB asset it took the bouncing `#=O=-` frames from 77 to 74. The cause is
  not redirects. curl's `--progress-bar` draws that indicator for as long as it
  does not know the transfer size, and over HTTPS that is the whole connect,
  TLS and header phase — 72 of 100 frames on a request with no redirects at all
  and a `Content-Length` present. So curl's meter is off now and the installer
  draws its own bar from the size the `HEAD` already reports: zero bouncing
  frames, a bar that moves from 0% to 100%, and a failed download still fails
  the install rather than showing a finished bar over a truncated file.

## [0.12.0] - 2026-09-13

The store learns the shape of the code in it, and search uses it.

### Added

- **A code graph, extracted on the same pass that re-embeds a file.** Symbols
  and the `defines`, `calls`, `imports`, `references` and `contains` edges
  between them, read out of the syntax tree by tree-sitter, for Rust,
  TypeScript, Python, Go, Java and C. It hangs off the blake3 hash-change path
  that already drives re-embedding, so an edit under `semlith start` updates the
  graph in the same pass that updates the vectors. There is no build step and no
  artifact that can quietly go stale — which is the failure every snapshot-based
  code graph has, and the reason this one is not a snapshot.
- **Extracted or inferred, on every edge.** A call whose name the file also
  imports was resolved by the file itself; a bare name match was not. Two
  functions called `new` in different modules is the normal case in real code,
  not a corner case, so the difference is recorded and shown everywhere an edge
  appears. Nothing presents the second as the first.
- **`semlith symbol`, `semlith neighbors`, `semlith path`, `semlith impact`**, and
  the same four as the MCP tools `semlith_symbol`, `semlith_neighbors`,
  `semlith_path` and `semlith_impact`. Where a symbol is defined; what calls it
  and what it calls; the shortest chain of edges between two symbols; and what
  breaks if this one changes. `impact` is the answer to the most expensive
  mistake an agent makes on a real codebase — patching the call site that was
  reported and leaving every sibling caller broken.
- **Graph-aware search.** The top vector and keyword hits are mapped to the
  symbols in them, expanded one hop, and the chunks those neighbours live in
  become a third ranked list fused at the same weight as the other two. It costs
  no embedding and no model call, and it reaches the case neither existing list
  can: a concept spread across files that share no vocabulary. Every hit now
  says which lists found it — `v`, `f`, `g` on the command line — so a result the
  graph alone reached is legible as a neighbour of a match rather than a match.
- **A Graph page and an Impact page in the portal.** The graph on a canvas with
  a force layout that can be paused, panned, zoomed and dragged, beside a rail
  carrying the selected symbol's callers, callees and chunks. Reverse
  reachability as a table and as rings by hop, with a path finder. Both are free
  on every tier, now and after 0.13.0 makes the product paid.
- **An opt-in retrieval ledger.** `semlith start --ledger` records every query an
  agent ran into a hash-chained `retrievals` table inside the store: the query,
  the client, the hits, the excerpt tokens they actually read and the whole-file
  tokens a grep loop would have cost. Each row carries the hash of the row
  before it, so an edited or deleted row is detectable rather than merely
  unlikely. `semlith ledger --last N` prints it, the Ledger page shows the
  totals and the measured ratio, and none of it needs a key or leaves the
  machine. It is off unless asked for: a local tool that starts recording what
  you searched for without being told to is not meaningfully different from one
  that phones home.

### Fixed

- **The install script's progress bar.** `curl -L --progress-bar` draws a bar for
  every hop of a redirect, and a hop carries no content length, so curl fell
  back to its bouncing `#=O=-` spinner. A GitHub release URL always redirects,
  so every install showed that before the real bar arrived, and a working
  download looked like line noise. The final URL is resolved first, which gives
  one request against a known size and one bar from 0% to 100%.

### Notes

- **Existing stores open unchanged.** `format_version` stays 2 and nothing
  migrates: the new tables are created by the same `IF NOT EXISTS` batch every
  open already runs, so a store written by 0.11.0 opens with an empty graph and
  fills it on the next `index` pass, and a store written by 0.12.0 still opens
  under 0.11.0. Both directions are tested against the released 0.11.0 binary.
  `docs/compatibility.md` explains why the number did not move.
- **Measured.** Extraction costs 98 ms over 589 KiB of source producing 757
  symbols and 5,536 edges — 0.17% of the 58 s that corpus takes to index, because
  embedding dominates and tree-sitter is fast. The six grammars add 4.6 MB to
  the binary, 38.6 MB to 43.2 MB.
- **Six languages carry edges.** Everything else is searchable exactly as
  before, with no symbols. The About page says which is which.
- **Everything in this release is free on every tier, permanently.** 0.13.0
  introduces paid tiers by adding views above these, never by locking one.

## [0.11.0] - 2026-09-12

The rest of a real archive, and a way to put something into a store that was
never a file on the disk.

### Added

- **EPUB, RTF, `.eml` and `.mbox` readers.** A book is read as its chapters in
  the order the spine gives, which is not the order the filenames sort in — so a
  book no longer opens on its copyright page. An RTF document is read as the
  text a word processor would show, with font and colour tables, style sheets,
  embedded pictures and revision metadata skipped whole and `\'hh` and `\uN`
  escapes decoded. A message is read as its `From`, `To`, `Cc`, `Date` and
  `Subject` followed by its body: the `text/plain` part of a multipart, or the
  HTML part run through the existing HTML reader when there is no plain one.
  Headers are unfolded and RFC 2047 encoded-words decoded, so a subject with an
  accent in it is searchable by the word rather than by `=?utf-8?Q?`. An
  attachment is named and never decoded. An `.mbox` is every message in the
  file, each one marked with its own subject. None of it adds a dependency: an
  EPUB is a ZIP of XHTML, so it costs the archive reader and the HTML scanner
  that were already here, and the other two are hand-written scanners.
- **`semlith add <URL>`**, on the CLI, as the `semlith_add` MCP tool, and as a
  URL field on the portal's Index page. Fetches one https URL into the store's
  own `downloads/` directory and indexes it through the readers that already
  exist — a web page, a PDF such as an arXiv paper, or a file on GitHub, whose
  `/blob/` link is rewritten to the raw file it displays. The directory is
  registered as a root of the store, so `semlith start` keeps what was added
  current.
- **Twenty-one more languages for `--lang`**: zig, dart, elixir, terraform,
  powershell, julia, fortran, r, perl, dockerfile, makefile, vue, svelte, proto,
  graphql, nix, clojure, erlang, elm, groovy and objective-c, bringing the table
  to 46. Two of them have no extension to match on, so a language may now be
  identified by filename as well: `--lang dockerfile` finds a bare `Dockerfile`
  and a `Dockerfile.prod`, and `--lang makefile` finds `Makefile` and
  `GNUmakefile`.
- **A Languages view in the portal**, listing all 46 with the extensions and
  filenames that make each one up, read from the same table the filter uses.

### Changed

- The portal's Files view shows a language for a file that has no extension,
  where it previously showed nothing.

### Security

- `semlith add` is the first outbound connection semlith makes that is not the
  model download or `semlith upgrade`, and it holds to the same rules. It is
  https-only and refuses plain http rather than silently upgrading it; it
  refuses a redirect that leaves https and a chain longer than five; it caps a
  body at 32 MiB; it refuses a content type no reader handles, cross-checking
  the declared type against the first bytes; it never sends a credential; and
  under `--airgap` it refuses before a socket is opened. The path it writes to
  is derived from the URL, so escapes are decoded before the path is split,
  every segment is sanitised, and the result is checked to be inside the
  downloads directory before anything is written. Nothing is crawled and nothing
  is re-fetched. See [#50](https://github.com/semlith/semlith/issues/50).

## [0.10.0] - 2026-09-12

One command on a fresh machine. No Rust toolchain, no package manager, nothing
installed first — and on Linux, no OpenBLAS and no OpenSSL either, because the
binary no longer needs them.

### Added

- **`install.sh` and `install.ps1`**, at the root of the `main` branch and
  reachable from `raw.githubusercontent.com`. Each detects the machine's
  target, resolves the newest release through the `releases/latest` redirect
  (`SEMLITH_VERSION` pins one instead), downloads the archive with a progress
  bar, verifies it against the release's `SHA256SUMS` before writing anything,
  unpacks into `~/.semlith/bin`, and hands off to `semlith setup`. A failed
  download or a bad checksum leaves nothing behind: no bin directory, no
  partial file outside a temp directory removed on exit. Intel macOS gets the
  ONNX Runtime explanation and no partial install.
- **`semlith setup [--yes]`**: a guided terminal flow that puts
  `~/.semlith/bin` on `PATH` for your shell, pre-downloads the embedding model
  so the first `index` is not a silent wait, registers semlith with the agents
  you pick — Claude Code through its own CLI, every other client by printing
  the stanza and the config path — and ends by running the installed binary's
  `--version`. Every step is idempotent and reports what is already done, so it
  is the repair command as well as the install one. `--yes` takes the default
  at every prompt and needs no terminal, so a script or an agent can install
  semlith unattended.
- **`semlith upgrade [--check] [--version <tag>] [--airgap]`**: fetches the
  newest release for this machine, verifies it against `SHA256SUMS`, and swaps
  the binary by renaming the old one aside and the new one into place — the one
  sequence that works on Linux, macOS and Windows alike. `--check` prints both
  versions, changes nothing, and exits 10 when an upgrade exists. It refuses
  under `--airgap` before opening a connection, refuses a binary it did not
  install, and never runs unprompted: no startup check, no timer, no banner.
- **Portal parity.** The welcome screen and the Agents view gain an "Install and
  setup" panel: both one-liners with copy buttons, a row per setup step with
  what it found, whether the bin directory is on `PATH`, the installed version,
  and Check-for-updates and Install buttons that only ever fire on a click. The
  panel reads `/api/setup`, which is `setup::status()` — the same function the
  terminal prints — so the page cannot claim `PATH` is set up when it is not.

### Changed

- **The Linux binary needs nothing but glibc and libstdc++.** fastembed moves to
  `ort-download-binaries-rustls-tls` and hf-hub to its ureq-only feature set,
  which takes reqwest, tokio and native-tls out of the build entirely — 607
  lines leave `Cargo.lock`. The release workflow now reads the built binary's
  `NEEDED` entries with `readelf`, prints them, and fails on `libssl`,
  `libcrypto` or `libopenblas`, so this cannot regress quietly.
- **The OpenBLAS instruction is gone** from the README, `CONTRIBUTING.md`,
  `AGENTS.md` and both workflows. turbovec 1.0.0 dropped its BLAS dependency and
  the documentation had not caught up: the step had been unnecessary for a
  release already.
- **The README's Install section leads with the two one-liners.** Homebrew,
  winget and Scoop are listed as coming; `cargo install` and the release archive
  are kept below as alternatives. The release notes' install text is now lifted
  out of the README between markers, so the two cannot drift apart.
- **One new runtime crate**: cliclack 0.5.6, for the prompts, the spinner and
  the progress bar. `sha2` and `ureq` are now named directly and were already in
  the tree through hf-hub, so neither adds anything to the build.

### Documentation

- `docs/compatibility.md` now covers the install script URLs and the
  environment variables they honour, the release archive layout both they and
  `semlith upgrade` read, `semlith setup --yes`, and `semlith upgrade --check`'s
  exit codes. `SEMLITH_RELEASES_ORIGIN` is listed as explicitly not covered: it
  redirects the release lookup for the tests and is not a way to self-host.

## [0.9.0] - 2026-09-12

A store no longer has to live inside the repository it indexes, and a portal in
your browser shows you what is in it. `semlith start` is one process that owns
every store, keeps them current as you save, and serves that page on
`127.0.0.1` — which also ends the choice between a watcher keeping a store
current and an agent being able to write to it.

### Added

- **A store home.** `semlith index` with no flags now creates
  `~/.semlith/stores/<name>` and records the directory it indexed in
  `~/.semlith/registry.json`. `SEMLITH_HOME` moves it; `--name` names a store
  explicitly. A store can no longer land in a tracked tree by default.
- **`semlith adopt <dir>`** moves an existing store into the home and registers
  it — a rename where it can be, a verified copy where it cannot, and no
  re-embedding either way. `--root` re-points a registered store whose corpus
  moved.
- **`semlith start`**: takes every registered store's write lock, runs the
  watcher over their roots, and serves a portal on `127.0.0.1:7365`. `--port`
  and `SEMLITH_PORT` change the port; nothing changes the address. If the port
  is taken it exits saying so rather than picking another, because the URL is
  meant to be a bookmark.
- **The portal**, compiled into the binary: a first-run welcome screen, and
  Stores, Files, Search, Index, Agents, Privacy and About. It shows each store's
  counts and the watcher's live event feed, the indexed files with the CLI's
  `path`/`ext`/`lang` filters and the reader that parsed each one, the same
  fused search the CLI and the MCP tools run, a folder picker whose index run
  streams progress as it happens, the client stanza for every documented agent,
  and a Privacy page with a packet-capture recipe.
- **`semlith mcp` forwards to a running daemon.** An agent's `semlith_index`
  used to be refused for as long as a watcher held the store; now the daemon is
  the writer and the agent is a client of it. Client configuration does not
  change, and with no daemon running `semlith mcp` behaves exactly as 0.8.0 did.
- **`--airgap`**, and `SEMLITH_AIRGAP=1`, on `index` and `start`: refuse to
  download model weights and exit naming the cache path, so a machine that
  pre-seeded `SEMLITH_MODEL_CACHE` can prove nothing was fetched.
- **A token rotate route**, behind the Privacy page's button: the old token is
  refused from the next request.

### Changed

- **Client stanzas lost their paths.** Every README stanza is now `semlith mcp`
  with no arguments, because the server opens every registered store plus a
  `.semlith` beside the working directory. Index a second repository and the
  agent that is already configured can search it, with no file to edit.
  `--store` still works and still wins when given.
- `semlith mcp` with no flags opens every registered store rather than
  `./.semlith` alone.
- The out-of-scope note in `AGENTS.md` and `docs/architecture.md` now states the
  loopback rule instead of forbidding a server, per
  [#41](https://github.com/semlith/semlith/issues/41). A CLI command or MCP tool
  is not done until its portal view exists, and `tests/portal.rs` is the gate.
- `turbovec` 0.9.0 → 1.0.0: `search_with_allowlist` returns a `Result` instead
  of panicking on an id the index does not hold, which is the better contract
  for a shard that legitimately lacks an id its range covers.
- `fastembed` 6.0.1 → 6.0.3.
- Three further Dependabot updates that landed on `develop` while this release
  was in flight and are merged into it rather than shipped separately: a
  patch-updates group of two crates, `Swatinem/rust-cache` in both workflows,
  and `softprops/action-gh-release` 3.0.2 → 3.0.3. Both actions stay SHA-pinned
  with their trailing version comment.
- `chacha20` in the lockfile, which had been yanked upstream. It arrives through
  `pdf-extract` → `lopdf` → `rand` and nothing here calls it, but
  `cargo install --locked` would otherwise pull a yanked crate.

### Security

- The portal binds `127.0.0.1` only, with no flag to change it. Every request
  needs a per-run token carried in a `SameSite=Strict; HttpOnly` cookie — 401
  and an empty body without it. The `Host` header must be `localhost`,
  `127.0.0.1` or `::1`, or the request gets 400. Every response carries a
  `Content-Security-Policy` allowing only `'self'`, and no CORS header is
  emitted anywhere. Each of those is asserted by a test.

### Dependencies

- **No new crate.** The HTTP server, the HTTP client the proxy uses, and the
  portal are all written against `std`, so the dependency count is unchanged at
  14 direct dependencies plus one dev-dependency — the same list 0.8.0 shipped,
  with `turbovec` and `fastembed` bumped. The portal adds 149 KB of embedded
  assets — HTML, CSS, JavaScript and five IBM Plex latin faces under the SIL
  OFL 1.1 — against a 1 MiB budget a test enforces.

## [0.8.0] - 2026-07-30

The documents a corpus is actually made of. A notebook, a Word file, a slide
deck, a spreadsheet or an HTML page is now read as the text a person opening it
would see, rather than as its markup, its JSON, or not at all.

### Added

- **Nine more formats, behind the same extraction the PDF reader already sat
  behind.** No new command and no new flag: `semlith index` reads them because
  it walked past them.

  | Extension | What is taken from it | Markers |
  | --- | --- | --- |
  | `.ipynb` | Every cell in notebook order, source and outputs. Stream output and a result's `text/plain` are kept, truncated at 2000 characters each; images, widgets and other MIME types are dropped. | `# Cell 3 (code)`, `# Output:` |
  | `.html`, `.htm` | The page's text. Tags go, `<script>` and `<style>` contents go with them, character entities are decoded. | — |
  | `.docx` | Paragraphs in document order, one per line; a table row's cells tab-separated. | — |
  | `.pptx` | Each slide's text, slides in numeric order. Speaker notes are not included. | `# Slide 11` |
  | `.xlsx` | Each sheet in workbook order, a line per row, tab-separated cells, shared and inline strings resolved. | `## Sheet: Q3 Notes` |
  | `.odt`, `.odp`, `.ods` | The same, from OpenDocument's `content.xml`. | `# Slide 2 (Intro)`, `## Sheet: Q3 Notes` |

  A marker exists wherever a format has a division a line number cannot
  express, so an excerpt says which slide or which cell it came from.

  Extracted HTML keeps every newline the source had, including the ones inside
  the tags that were removed. That is what keeps a hit's `file:line` range
  pointing at the line of the file on disk where the sentence lives, rather than
  at a line number that only exists after extraction. A spreadsheet is indexed
  as its cached cell values; formulas are not evaluated.

- **A cap on what an archive may decompress to: 32 MiB of text.** Six of the
  nine formats are ZIP archives, and the existing 8 MiB file cap only bounds the
  compressed file on disk.

  *Why.* A few hundred kilobytes of zeros expand to gigabytes. Without a bound
  on what comes out, the size of a run's largest allocation would be chosen by
  whoever wrote the file rather than by semlith. 32 MiB is more text than any
  real document holds and small enough that reaching it is a decision.

- **`zip` 8.6.0** (MIT), with `default-features = false` and only
  `deflate-flate2` — the decompressor and nothing else, no compressors and no
  ciphers.

### Changed

- **HTML files and notebooks are indexed differently than before, not just
  additionally.** Under 0.7.0 an `.html` file was indexed as its raw markup and
  a notebook as its raw JSON; both are now indexed as their text.

  *Why.* A notebook chunk was mostly `"cell_type"`, `"outputs"` and escaped
  newlines, and an HTML chunk was mostly attributes and closing tags. What a
  person is searching for is the third line of the fourth cell, or the sentence
  in the paragraph — so that is what gets embedded.

  *What to do.* Nothing for files indexed from now on. But indexing is keyed on
  content hashes, so a file already in a store is not re-read while its contents
  are unchanged: an existing store keeps its old markup and JSON chunks for
  those files until they change or the store is rebuilt. To convert one file,
  `semlith forget <PATH>` — it drops the file's chunks and its recorded hash, so
  the next `index` run reads it afresh. It takes exactly one path and no globs,
  so for a corpus of them the shorter route is to delete the store directory and
  index again. Neither is urgent; a stale chunk is worse retrieval, not a
  broken store.

- **A document that cannot be read is a skipped file.** Corrupt, truncated,
  password-protected, or over a cap — it lands in the run's `skipped` total,
  exactly as an unreadable PDF has since 0.1.0, and the run still exits 0 with
  every other file indexed. A panic inside an extractor is caught and becomes a
  skipped file too: these readers sit downstream of a decompressor and a
  document somebody else wrote, and one bad file should not end a run that is
  minutes from finishing.

- **No store format change and no migration.** `format_version` is untouched, a
  0.8.0 store is readable by 0.7.0 and a 0.7.0 store by 0.8.0, and downgrading
  loses the new formats and nothing else.

## [0.7.0] - 2026-07-30

A corpus larger than a repository. The first index of one says where it has got
to, survives being interrupted, and searching it costs a bounded amount of
memory rather than an amount that grows with the corpus.

### Changed

- **Breaking: a store created by 0.7.0 has a new layout, and an older semlith
  will not read it.** Vectors now live in a directory of fixed-size shards under
  `index/` instead of a single `index.tv`, recorded as `format_version` 2.

  *Why.* Everything else in this release follows from it. A search can hold a
  few shards instead of the whole corpus; a change to one file rewrites one
  shard instead of the entire index, which is what `semlith watch` does on every
  save; and an index run can make its work durable as it goes, because a
  checkpoint is a shard that has landed. None of the three is possible while the
  vectors are one file that must be written whole.

  *What to do.* Nothing, for a store you already have: 0.7.0 reads, searches and
  indexes into every store written before it exactly as 0.6.0 did, leaves it on
  its single `index.tv`, and does not touch its `format_version`. Only stores
  created by 0.7.0 are sharded. To move an existing store onto the new layout,
  delete the store directory and index it again — the vectors in an `index.tv`
  are quantized and cannot be split back out, so there is no migration that
  would not re-embed the corpus anyway. For a large corpus that costs hours;
  there is no hurry, and nothing breaks if you never do it.

  *If you downgrade.* A 0.6.0 binary meeting a 0.7.0 store refuses it, naming
  both format numbers. A 0.5.0 binary is older than `format_version` and has
  nothing to check, so it reads such a store as an empty corpus — if you keep a
  0.5.0 binary around, do not point it at a store 0.7.0 created.

### Added

- **A resident-memory budget for the vector index.** `SEMLITH_INDEX_MEMORY`, in
  megabytes, defaults to 512. A sharded store holds at most as many shards as
  that allows, putting down the coldest one to make room, and says on stderr
  when it has had to. `semlith stats` reports the shard count and the budget, so
  what a store costs to search is legible before searching it.

- **Checkpointed indexing.** A sharded store makes its vectors durable every
  thirty seconds and only then records the files they cover as indexed — in that
  order, because a hash written ahead of its vectors is a file the next run
  believes it has. An index run killed partway keeps everything committed so
  far, answers searches from it immediately, and the next run walks past it
  rather than starting again. Under 0.6.0 the same interruption left nothing.

- **Progress that predicts.** `semlith index` now says how many files of how
  many it has walked, the chunks per second it is managing, and an estimate of
  what is left. A run that resumes says how many files it skipped as already
  indexed.

- **Opening a store loads no vectors.** `stats`, `files` and `forget` read the
  index only if they must, and an MCP server holding several stores open for an
  agent costs nothing for them until something is searched.

### Performance

- Changing one file in a sharded store rewrites one shard rather than the whole
  index — the difference between watching a monorepo and not being able to.
- Search latency is unchanged for a store that fits inside its budget. A store
  larger than its budget pays to read shards back on each query; that cost is
  measured and reported rather than smoothed over.

## [0.6.0] - 2026-07-30

semlith works from whichever agent you already use, the MCP tools cover the
store rather than a fifth of it, and what counts as stable is written down.

### Fixed

- **The server no longer claims protocol revisions it cannot speak.**
  `initialize` answered with whatever `protocolVersion` the client asked for, so
  a client on `2026-07-28` was told semlith spoke `2026-07-28` — a revision that
  had removed the very handshake it was answering. semlith now holds a list of
  the revisions it implements and answers with the requested one when it is on
  that list, or with the newest one it does implement when it is not.

### Added

- **MCP `2026-07-28`, the stateless revision, alongside the handshake.** That
  revision deleted `initialize`, made `server/discover` mandatory and moved the
  protocol version onto every request. semlith serves both eras, deciding per
  message rather than per connection: a request carrying
  `_meta.io.modelcontextprotocol/protocolVersion` gets `resultType`, server
  identity in `_meta`, and `ttlMs`/`cacheScope` on `tools/list`; anything else
  gets exactly the answer 0.5.0 sent. A revision semlith does not implement
  comes back as `-32022` naming the ones it does.

  The advertised list is `2026-07-28`, `2025-11-25`, `2025-06-18`,
  `2024-11-05`, and every entry has a recorded session in `tests/mcp.rs`
  proving it. `2025-03-26` is deliberately absent: it is the only revision that
  required JSON-RPC batching.

- **Three more tools, so the MCP surface covers what the CLI does.**

  | Tool | What it does |
  | --- | --- |
  | `semlith_files` | Which files are indexed, narrowed by the same `path`/`ext`/`lang`/`store`, capped and saying how many it left out. |
  | `semlith_index` | Index a path into an open store, so a corpus becomes searchable mid-conversation. |
  | `semlith_forget` | Drop one file from a store. The file on disk is untouched. |

  `semlith_files` exists because an agent that cannot ask "is this indexed"
  reads an empty search result as "the corpus does not discuss this".

  Both writers take the store's lock for the call and give it back, so a store
  a `semlith watch` is holding comes back as a tool error naming the holder
  rather than as a corrupted index. With more than one store open they require
  a `store` argument: a store takes one writer, and there is no "the" store to
  guess at.

  `semlith_index` works to a wall-clock budget — 45 seconds, under the
  60-second tool timeout clients default to — then returns what it reached and
  how much is left. Calling it again continues rather than restarting, because
  indexing has been keyed on content hashes since 0.1.0. It never creates a
  store, so no tool call can contain a model download.

- **A setup stanza for every MCP client in common use**, each verified against
  that client's own documentation: Claude Code, Claude Desktop, Codex, GitHub
  Copilot in VS Code, Copilot CLI, Cursor, Windsurf, Zed, Gemini CLI,
  JetBrains, Cline and Goose. `tests/clients.rs` extracts every stanza from the
  README, parses it, and runs its command line against a real server, so a flag
  renamed in the code and not in the README fails the build rather than
  somebody's first attempt.

- **[`docs/compatibility.md`](docs/compatibility.md)** — which surfaces are a
  contract (CLI commands and flags, MCP tool names and schemas, the advertised
  protocol revisions, the store on disk, the `lib.rs` API) and which are free to
  change (ranking scores, human-readable output, stderr, the default model,
  additive JSON fields). It is explicit that 0.x is what backs the promise.

- **`format_version` in the store's meta table.** A store created by 0.6.0
  records format 1; a store written before the key existed is read as format 1
  and never rewritten; a store from a format the binary does not know is refused
  naming both numbers instead of misread. Written down now, while nothing has
  changed, so the first change that does happen fails loudly.

- **`SEMLITH_MCP_INDEX_BUDGET`**, in seconds, for a client whose tool timeout is
  not the usual one.

- **The MCP server says what it is on stderr** — the stores it opened, and the
  revision it negotiated when that differs from what was asked. stdout is the
  protocol; stderr is the only channel a stdio client captures, and "the agent
  sees no tools" was otherwise undiagnosable.

### Changed

- **`semlith forget` takes the store's write lock.** It rewrites `index.tv`
  exactly as indexing does, so it is a writer and now waits its turn like one.
  A `forget` that ran while `semlith watch` was saving could leave the index and
  the database disagreeing about which chunks exist. It now exits non-zero
  naming the holder instead.

### Performance

Measured on an Apple M1:

- The tool list grew from 2220 bytes for two tools to 4790 for five — roughly
  555 estimated tokens to 1197, measured through both release binaries. It is
  loaded into an agent's context once per session whether or not a tool is
  called, so three more tools cost about 640 tokens a session.
- An MCP server at rest holds no store lock: `semlith index` in another terminal
  against the store an open server is serving succeeds.

## [0.5.0] - 2026-07-30

One query across several stores, so an agent working across repositories asks
one question instead of one per repository.

### Added

- **`--store` is repeatable on the read commands.** `search`, `stats`, `files`
  and `mcp` cover every store named:

  ```sh
  semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith
  ```

  ```
  1. 0.033  [api] src/lock.rs:14-31
  2. 0.032  [cli] src/main.rs:96-104
  2 hits in 4.1ms across 2 stores: api 1, cli 1
  ```

  `-k` stays global — ten results over three stores is ten results. Filters
  apply to every store, and a filter matching files in only one of them returns
  that one's hits rather than reporting that nothing matched. Stores whose
  embedding models differ can be searched together: each embeds the query with
  its own model, and nothing downstream compares two models' numbers.

- **Every hit says which store it came from**, in the text output, in `--json`
  as a `store` field, and over MCP. A store is named after the directory holding
  it, so `../api/.semlith` is `api`; two stores that would collide get their
  paths instead.

- **`SEMLITH_STORE` accepts a list**, split the way `PATH` is, so an MCP server
  definition can name several stores without a wrapper script.

- **One MCP server, several stores.** `semlith_search` gained an optional
  `store` array that narrows a query, the tool description lists the stores that
  are open, an unknown name comes back as a tool error naming the real ones, and
  `semlith_stats` reports one line per store.

  Adding a store is cheap: measured on three 300-file stores sharing a model,
  one query embed per search rather than one per store, a median 3.4ms for one
  store against 4.0ms for three, and a server holding 137 MB whether it was
  opened on one store or on three — one loaded model, not three.

### Changed

- **A read command refuses a store path that is not already a store**, instead
  of creating one. `search`, `stats`, `files` and `mcp` exit non-zero naming the
  path. Previously a mistyped `--store` became an empty store that answered
  every question with nothing — invisible in a multi-store query, where the
  other stores still return hits. `index` still creates the store it is given.
- **`index`, `watch` and `forget` take exactly one `--store`** and exit non-zero
  if given more. They write, and a store has one writer.

### Notes

No schema change and no index format change: a 0.4.0 store is searched by 0.5.0
with no re-index, and a store written by 0.5.0 is read by the 0.4.0 binary. A
single-store invocation prints and serializes exactly what 0.4.0 did — the store
label is absent when there is nothing to tell apart.

## [0.4.0] - 2026-07-29

Keeps a store current while you work: files are re-embedded as they are saved,
and an agent already connected over MCP sees the change without restarting.

### Added

- **`semlith watch [PATHS...]`.** Stays running and re-embeds files as they are
  saved, so `semlith index` stops being something you have to remember.

  ```sh
  semlith watch ~/notes ./src
  ```

  It begins with the same incremental pass `index` runs — so whatever changed
  while nothing was watching is caught up — and then waits on filesystem events
  rather than polling. Measured on a 1000-file corpus: no measurable CPU over a
  60-second idle window, and about a second from a save to that text being
  searchable.

  New files are indexed, deleted files lose their chunks and vectors, and a
  rename moves the file rather than duplicating it. An editor that saves by
  writing a temp file and renaming it over the original is treated as an edit,
  not as a deletion followed by a new file. Ignore rules come from the same walk
  `index` uses, so `.gitignore`, hidden files and the store's own directory are
  skipped by construction rather than by a second set of rules.

  Events are batched until things go quiet for `--debounce` milliseconds (500 by
  default), so one save — or a formatter rewriting a file three times — costs one
  re-embed and one index write.

- **A long-running reader now sees another process's writes.** The store counts
  index rewrites, and a search reloads the vector index when that count has
  moved. In practice: run `semlith watch` beside your agent's `semlith mcp`
  server, and the agent's answers track your working tree with nothing
  restarted. The check is one SQLite read per search.

- **Ctrl-C stops a watcher cleanly.** `SIGINT` and `SIGTERM` end it at a batch
  boundary, so `index.tv` is written whole or not at all and no temp index is
  left behind. A second signal kills it outright. Unix only; on Windows the
  default terminate-immediately behaviour applies.

### Changed

- `semlith watch` holds the store's write lock for as long as it runs. A store
  has one writer, so `semlith index` against a watched store exits non-zero and
  names the watcher's process. Searching is unaffected.
- An indexing pass that changes nothing no longer rewrites `index.tv`.

### Compatibility

No schema change and no index format change. A 0.3.0 store is watched without
re-indexing, and a store written by 0.4.0 opens, searches and indexes under
0.3.0 — both verified against the 0.3.0 release binary. The only addition to the
store is one row in the existing `meta` table.

## [0.3.0] - 2026-07-29

Narrows a search to part of a corpus, so one store per repository can answer a
question about one subsystem.

### Added

- **`--path`, `--ext` and `--lang` on `semlith search`.** Each is repeatable.
  Repeats union, kinds intersect: `--ext rs --ext toml` is "Rust or TOML",
  `--path 'src/**' --ext md` is "Markdown, under `src`".

  ```sh
  semlith search "how does retry backoff work" --path 'src/http/**'
  semlith search "how does retry backoff work" --lang rust
  ```

  The filter is applied *before* either half of the search picks its results.
  Asking for eight hits inside a subdirectory returns the eight best hits in
  that subdirectory, not whatever survives filtering the eight best hits in the
  repository — which for a small subdirectory is usually nothing. Concretely:
  the vector index is scanned under a turbovec allowlist and the FTS5 query
  carries the same path predicate, both derived from one id-selection query, so
  rank fusion never sees a chunk one half was forbidden to return.

  Path patterns are SQLite `GLOB`. A pattern that does not start with `/` is
  anchored as `*/<pattern>` against the stored absolute path, so `src/**`
  works from any working directory; an absolute pattern means exactly itself.
  `*` crosses `/`, so `src/*` already reaches the whole subtree. Matching
  ignores case, so `--ext md` finds `README.MD`.

  A filter that selects no indexed file reports that, rather than reporting
  that the corpus does not match the query.

- **The `semlith_search` MCP tool takes the same filters**, as optional `path`,
  `ext` and `lang` arrays, which is the point of the release: an agent working
  on one subsystem can scope its question to that subsystem.

- **`semlith languages`** prints the language names `--lang` accepts and the
  extensions each covers. An unrecognised name is an error naming this command,
  not a silent empty result.

### Notes

No store format change. `files.path` has been recorded since 0.1.0, so
filtering works on an existing store with no re-indexing, and a store written
by 0.3.0 is readable by 0.2.0.

## [0.2.0] - 2026-07-29

Removes three of the four limitations 0.1.0 shipped with, cuts peak indexing
memory to a third, and changes the default embedding model.

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

### Performance

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM, over corpora of
mixed Rust, Markdown and TypeScript.

| store | warm query, p50 | indexing | peak RSS |
|---|---|---|---|
| 1.2k chunks | 2.7 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | 5.4 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | 22.7 ms | 23.5 chunks/sec | 595 MB |

- **Peak indexing memory is now roughly 600 MB and no longer grows with the
  corpus**, against ~1.7 GB in 0.1.0. Across a 1.2k, 9.9k and 105k chunk corpus
  it varies by 7 percent, and the largest corpus uses the least. Two causes:
  embedding batches were being flushed once per file rather than once per
  batch, so a single large file could hold thousands of chunks in memory; and
  the batch size itself was three times larger than it needed to be.
- **Indexing throughput is up from ~13 to ~23 chunks/sec**, from sizing the
  ONNX Runtime thread pool to the machine's performance cores.
- **Retrieval quality**, measured end to end through the binary over a
  6527-chunk corpus with 1809 self-labelled queries:

  | | code MRR@10 | docs MRR@10 |
  |---|---|---|
  | 0.1.0 (BGE-small, dense only) | 14.84 | 15.09 |
  | 0.2.0 (granite int8, dense only) | 16.00 | 14.08 |
  | **0.2.0 as shipped (granite int8 + FTS5)** | **17.90** | **15.62** |

  The model change alone would have regressed prose retrieval. Keyword fusion
  more than recovers it, which is why the two ship together.

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

[Unreleased]: https://github.com/semlith/semlith/compare/v0.13.0...HEAD
[0.13.0]: https://github.com/semlith/semlith/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/semlith/semlith/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/semlith/semlith/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/semlith/semlith/releases/tag/v0.10.0
[0.9.0]: https://github.com/semlith/semlith/releases/tag/v0.9.0
[0.8.0]: https://github.com/semlith/semlith/releases/tag/v0.8.0
[0.7.0]: https://github.com/semlith/semlith/releases/tag/v0.7.0
[0.6.0]: https://github.com/semlith/semlith/releases/tag/v0.6.0
[0.5.0]: https://github.com/semlith/semlith/releases/tag/v0.5.0
[0.4.0]: https://github.com/semlith/semlith/releases/tag/v0.4.0
[0.3.0]: https://github.com/semlith/semlith/releases/tag/v0.3.0
[0.2.0]: https://github.com/semlith/semlith/releases/tag/v0.2.0
[0.1.0]: https://github.com/semlith/semlith/releases/tag/v0.1.0
