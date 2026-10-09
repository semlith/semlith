# Compatibility

Which parts of semlith are a contract you can build against, which are free to
change, and what changed in each release. Read it if you wire semlith into a
script, an agent configuration or another crate, or before you upgrade.

## What is covered

These surfaces are the public contract. A change that breaks one is recorded as
a break.

| Surface | What is promised |
|---|---|
| CLI commands | The names and what each does: `index`, `start`, `key`, `adopt`, `trust`, `watch`, `search`, `brief`, `read`, `pattern`, `stats`, `files`, `forget`, `compact`, `refused`, `scan`, `drop`, `add`, `mcp`, `hook`, `setup`, `doctor`, `accel`, `upgrade`, `models`, `languages`, `ledger`, `prices`, `symbol`, `neighbors`, `impact`, `trace`, `report`, `schedule`, `cloud`, `path`. `__embed-worker` is hidden and not part of the interface. |
| CLI flags | Flag names, short forms and meanings, including the repeatable `--store`/`-s` on read commands and the single `--store` write commands take. A leading `!` on a `--path`, `--ext` or `--lang` value excludes, applied after the inclusions of its kind; a value without `!` means what it always meant. The same holds for the `path`, `ext` and `lang` fields of every MCP tool that takes them. |
| Environment | `SEMLITH_STORE` (a path-separator-delimited list, split the way `PATH` is), `SEMLITH_HOME`, `SEMLITH_PORT`, `SEMLITH_AIRGAP`, `SEMLITH_EMBED_THREADS`, `SEMLITH_MCP_INDEX_BUDGET`, `SEMLITH_INDEX_MEMORY`, `SEMLITH_INDEX_PARALLEL` (runs at once), `SEMLITH_ADD_ALLOW_PRIVATE`, the `SEMLITH_AGENT_KEY` an HTTP stanza names, `SEMLITH_LEDGER` (`0`, `off` or `false` stops recording on this machine), `SEMLITH_DEFAULT_IGNORES` (`0`, `off` or `false` turns off the built-in table of generated and vendored directories), `SEMLITH_ACCEL` (a comma list of lanes such as `cpu,gpu`; a lane it omits is off, and it takes precedence over the saved switches) and `SEMLITH_SESSION_IDLE_SECS` (how long a writer keeps an unused session, 60 by default). |
| Exit codes | Whether an outcome exits zero. A blocked index run exits non-zero; a search that finds nothing exits zero. `upgrade --check` exits 0 when current and 10 when a newer release exists, and changes nothing. `scan` exits non-zero while the store holds anything today's rules would refuse. `doctor` exits non-zero when something is wrong (a client that is not installed is not a fault). `doctor --gpu` exits non-zero when a lane fails its known-answer check, not for an unavailable lane. `accel` exits non-zero on a refusal. `hook` never exits non-zero. |
| `semlith hook` | Reads one `PreToolUse` event as JSON on stdin and writes the client's answer on stdout. It never blocks unless a blocking mode was asked for, and is silent for a file no registered store holds. |
| `semlith setup --yes` | Runs every step with its default and no prompt, so a script or an agent can install semlith unattended. |
| `semlith setup --file-managers` | Adds "Index with semlith" to the file manager — a Finder Quick Action in `~/Library/Services`, an Explorer verb under `HKCU\Software\Classes\{Directory,*}\shell\semlith` with a Send to entry, or a Nautilus script, a Dolphin service menu and a Thunar action — each running `semlith index` on the selected paths. `--no-file-managers` removes them; the two conflict. Without either, `setup` asks with no as the default, and `--yes` leaves them as they are. |
| MCP tool names | `semlith_search`, `semlith_brief`, `semlith_read`, `semlith_files`, `semlith_symbol`, `semlith_neighbors`, `semlith_impact`, `semlith_stats`, `semlith_index`, `semlith_add`, `semlith_forget`, `semlith_path`, `semlith_trace`, `semlith_pattern`, `semlith_languages`, `semlith_report`. `tools/list` advertises the first eight unless `SEMLITH_MCP_TOOLS=all`; the server answers all of them. |
| MCP input schemas | The arguments each tool accepts and their types. An existing argument does not change meaning or become required. |
| MCP handshake | `initialize` returns an `instructions` string, the same one `server/discover` returns. A client that ignores it is unaffected. |
| MCP protocol revisions | The server advertises `2026-07-28`, `2025-11-25`, `2025-06-18`, `2024-11-05`. Dropping one is a break. |
| The MCP endpoint over HTTP | `POST /mcp` on the daemon's port, authenticated by an `Authorization: Bearer` header carrying the agent key from `~/.semlith/agent.key`. The path, the header and the key's location are a contract, because a client's configuration names all three. The key opens `/mcp` and nothing else. |
| The portal's session credential | A `Semlith-Token` request header. A write also needs a JSON content type, and `Sec-Fetch-Site: same-origin` from any client that sends fetch metadata. |
| Where the daemon binds | `127.0.0.1` only. Widening it would be a break, and no flag will ever do it. |
| The default port | `7365`. A taken port is an error, not a reassignment. |
| The install scripts | `install.sh` and `install.ps1` stay at the root of the `main` branch, so the two `raw.githubusercontent.com` URLs in the README keep working. They honour `SEMLITH_VERSION`, `SEMLITH_HOME`, `SEMLITH_YES` and `SEMLITH_NO_SERVICE`, and verify the download against the release's `SHA256SUMS` before writing anything. If `semlith.com` ever serves the installers, it redirects to these URLs rather than replacing them. |
| Release archives | One archive per target, named `semlith-<tag>-<target>`, holding a directory of that name with the binary in it, and a `SHA256SUMS` asset beside them in GNU `sha256sum` format. Linux archives also hold `libonnxruntime.so`, and every file in an archive is unpacked. `semlith upgrade` and both scripts read that layout. |
| Store layout | A store directory holds `store.db` beside its vectors: `index.tv` in format 1, an `index/` directory of shards from format 2. Which binary reads which store is under [Store format](#store-format). |
| The formats that are read | The list in the README's *What gets indexed* only grows. What is extracted from a file is not covered. |
| Language names | The set `--lang` accepts only grows. A name that resolves today resolves tomorrow, to at least the files it resolves to now. Which extensions or filenames make up a name may grow. |
| What `semlith add` refuses | Plain `http`, a redirect that leaves `https`, a chain longer than five redirects, a body over 32 MiB, a content type no reader handles, an address not on the public internet (unless `SEMLITH_ADD_ALLOW_PRIVATE=1`), and any invocation under `--airgap`. Each exits non-zero and writes nothing. Relaxing any of them is a break. |
| Where `semlith add` writes | The `downloads/` directory inside the store, laid out by host. Never the user's working tree; a second fetch of the same name is suffixed, not overwritten. |
| Ledger row kinds | A row kind an older binary does not know (such as `raw-read`, written by `semlith hook` for a whole-file read of an indexed file) still verifies in its chain. |
| `src/lib.rs` | Documented, not frozen. The `Semlith` type, `Hit`, `IndexReport`, and the modules `chunk`, `embed`, `filter`, `fleet`, `lock`, `mcp`, `store`, `watch` are the supported surface, but the library API changes with the minor version. See [the honest version of the promise](#the-honest-version-of-the-promise). |

## What is not covered

Each of these is excluded because freezing it would freeze something semlith
should be free to improve.

- **The daemon's HTTP routes.** Everything under `/api/` is how the portal talks
  to the process that serves it, and both ship in one binary. Paths, shapes and
  status codes may change in any release. For a stable programmatic surface, use
  `--json` on the CLI or the MCP tools. Route changes are still listed below,
  because scripts read them.
- **The portal.** Its routes, markup, assets and appearance.
- **Tool-written state.** `~/.semlith/registry.json`, each store's
  `daemon.json`, `settings.json`, `queued.json`, `schedules.json`, `cloud.json`,
  a store's `runs.jsonl`, `cache/vectors.db` and the model cache's `accel/`
  directories. semlith writes them, there is no supported way to hand-edit them,
  a field semlith does not recognise may be dropped on the next write, and their
  shapes may change. An absent or unparseable file is the same as no file. None
  is a configuration file. A binary before 0.9.0 never reads `registry.json`, so
  reinstalling 0.8.0 loses the registry and nothing else.
- **The default store location.** A new store is created under
  `~/.semlith/stores/<name>`. A `.semlith` beside the corpus is still found
  before the home is consulted, and nothing moves until you run `semlith adopt`.
- **The default embedding model.** A store records the model it was built with
  and keeps it, so a new default applies only to stores created after the
  change. Name a model with `--model` when you create a store if you need a
  specific one.
- **Ranking scores and result order.** The `score` on a hit is a reciprocal
  rank fusion score. It orders results within one query and means nothing across
  queries or across versions. Any ranking improvement moves both. Do not assert
  on a score or on a position.
- **The text extracted from a file.** Marker lines, the order of pieces, what is
  dropped, where a paragraph ends — all may change, because each reader answers
  "what would a person see if they opened this?". A document read differently is
  re-chunked and re-embedded when the file next changes, so chunk text and line
  ranges move with it.
- **Human-readable stdout.** What `search` prints, how `stats` lines up, the
  wording of a summary. Parse `--json`.
- **stderr diagnostics.** Progress, warnings, the MCP server's notes about the
  revision it negotiated and the stores it opened.
- **Additive fields.** New keys may appear in `--json` output and new lines in
  MCP tool text. Existing keys keep their names, types and meanings. Read JSON by
  key and ignore what you do not recognise; a reader that switches on known
  values should pass unknown ones through.
- **The internal SQLite schema.** That there is a `store.db` is covered; its
  tables, columns, FTS5 configuration and indexes are not. Use the `store`
  module or the CLI.
- **Which clients `semlith setup` registers through their CLI** and which
  through a file. Client file formats and locations move between versions.
- **Test-only variables.** `SEMLITH_RELEASES_ORIGIN` points the release lookup at
  another host and is read only in a debug build, for `tests/install.rs` and
  `tests/upgrade.rs`; a release binary fetches only from `https://github.com`.
  `SEMLITH_CHECKPOINT_FILES` and `SEMLITH_EMBED_VARIANT` exist for tests.
- **Portal parity exemptions.** Every CLI command and MCP tool has a portal
  surface except those argued in `tests/portal.rs`. `semlith models` and
  `/api/models` have none: no page lists every model a store could be built
  with. `semlith models` prints the full list and `/api/models` answers.

## The honest version of the promise

semlith is 0.x. Under SemVer a 0.x minor bump may break anything, and this page
does not pretend otherwise.

The project's practice is that the surfaces under *What is covered* stay stable
across 0.x releases: a script, agent configuration or crate written against one
minor version keeps working on the next. That is a commitment about behaviour,
not a guarantee the version number carries. The CLI, the MCP tools and the store
format are the surfaces it most strongly covers.

The exception is the `src/lib.rs` API, which took breaking changes in 0.2.0 and
is the most likely to move again. A library consumer should pin an exact version
and read the CHANGELOG before upgrading.

Nothing has been decided about 1.0.

## Store format

The store's `meta` table carries `format_version`. A store without the key is
format 1.

| Format | What it says about the store | Written by |
|---|---|---|
| 1 | vectors in one `index.tv` | 0.1.0 through 0.6.0 |
| 2 | vectors in an `index/` directory of fixed-size shards | 0.7.0 through 0.21.0 |
| 3 | Markdown chunks cut at their headings, embedded with the heading path in front | 0.22.0 through 0.24.0 |
| 4 | code chunks embedded with the definition they sit inside | 0.25.0 and later |

**A binary refuses a store whose format is newer than it knows**, naming both
numbers. Misreading a newer store does not look like an error; it looks like a
corpus that stopped containing things. One sharp edge: 0.5.0 and earlier predate
the key, so they read a format-2 store as an empty corpus instead of refusing it.
Do not point a pre-0.6.0 binary at a store 0.7.0 or later created.

**Forwards, every store still opens.** A newer binary reads, searches and
indexes into an older store without re-embedding and without changing its
format number. A format-1 store is never migrated: its quantized vectors cannot
be split back out, so moving to shards means deleting the store and indexing
again.

**Formats 3 and 4 re-embed once.** They change what the model was shown, so half
a store each way would rank its own files unevenly and silently. The first
index pass under 0.22.0 (format 3) or 0.25.0 (format 4) that sweeps the whole
store re-chunks and re-embeds everything it walks and moves the format row at
the end; a pass over one directory leaves the rest on the old rule and the row
where it was. The run says on its first line that it is re-indexing and why.
Until that pass, the store answers as it did. Rolling back from format 4 means
re-indexing under the older binary.

**Added tables and columns do not move the number.** The schema is applied with
`CREATE TABLE IF NOT EXISTS` (and nullable `ALTER TABLE` additions) on every
open, while `format_version` is written only when a store is created; bumping it
would strand every existing store and make the previous release refuse stores
the new one had merely opened. The number guards the vector layout and what the
model was shown, where misreading is silent. An older binary never reads a table
or column it does not know (its `SELECT`s name their columns), so these
additions are compatible in both directions with no migration:

| Release | Added to a store | Before the next index pass |
|---|---|---|
| 0.12.0 | `symbols`, `edges` (the code graph) and `retrievals` (the ledger) | empty graph and ledger; the graph fills on the next pass, which re-reads file bytes |
| 0.13.0 | an `images` table and an `images/` vector directory | no images; an image vector needs the file's bytes |
| 0.15.0 | nullable `edges.hint`; `retrievals.session`, `tool`, `stale_hits`, `tokenizer` | NULL hints resolve as before, a tier less precise |
| 0.28.0 | a `variants` meta row counting chunks per model variant | no row |
| 0.30.0 | `refusals` and `acceptances` tables | empty |
| 0.31.0 | the `vectors_swapping` meta key, written during a compaction's swap and left `0` | — |
| 0.32.0 | new `variants` keys: `fp16-ane`, `fp16-coreml-gpu`, `fp16-trt`, `openvino`, `gguf-f16` | — |
| 0.33.0 | nullable `retrievals.client_version`, outside the hash chain as `query_id` is | — |
| 0.35.0 | a `runs.jsonl` file of finished runs, newest 50 kept | — |
| 0.37.0 | `retrievals.synced_at`, outside the chain | — |
| 0.38.0 | an index on the lowered file path, added on open | — |

The ledger's hash chain covers a row's fields, so it is versioned by the row:
`tool` is NULL on every row written before 0.15.0, and that selects the formula.
A store holding both kinds verifies end to end.

A store's `variants` row may hold fp16 vectors; an older binary's int8 queries
search them, and int8 and fp16 vectors of the same text agree at cosine 0.987.
The rescoring model is read at query time and never written into a store.

## Changes by release

Each entry lists what a script, an agent or a stored file could notice. A
**break** is a change to a covered surface. Route changes are listed even
though routes are not covered.

### 0.13.0

- **Added:** `key show`, `key rotate`, `start --no-mcp-http`; the `/mcp`
  endpoint; an `images` table (above).
- **Break:** `semlith impact`, the `semlith_impact` MCP tool and
  `GET /api/impact` were removed (restored in 0.26.0), taking the tool count from
  ten to nine. A saved prompt or tool allow-list naming it got an unknown-tool
  error, and a script calling it got an unrecognised-subcommand error. There was
  no shim, because a tool that answers with an apology is one an agent keeps
  calling. `semlith neighbors <symbol>` and `semlith path <from> <to>` cover the
  one-hop and reachability questions.

### 0.14.0

Four breaks, all narrowing what 0.13.0 allowed after a security audit found that
a browser tab on another local port, and a cloned repository, could each act as
the user. Also added: `trust <dir>`, `trust --list`, `index --include-secrets`.

| Break | What fails now | The way back |
|---|---|---|
| An unregistered local `.semlith` is not opened | `search`, `stats`, `files`, `forget`, `index` and `semlith mcp` in a directory holding a `.semlith` semlith did not create exit non-zero, naming the store and both ways forward | `semlith trust <dir>` records it and moves nothing; `semlith adopt <dir>` moves it into the store home. `--store` and `SEMLITH_STORE` still open anything. Stores `semlith index` made are unaffected |
| `semlith_index` and the portal index inside a boundary | `semlith_index`, `POST /api/index`, `/api/root` and `/api/adopt` refuse a path outside the store's registered roots, outside the store's own directory when it is a `.semlith` beside its corpus, or outside the home directory, and any credential directory or name (then `.env`, `.env.*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`, `id_rsa*`, `id_ed25519*`, `*credentials*`, `*secret*`, `*.tfstate`, `*.kdbx`; the full current list is in [security.md](security.md)). The hidden-file rule also applies to a path named explicitly. Each refusal names its rule | `semlith index` on the command line keeps the deny-list but no root boundary; `--include-secrets` turns the deny-list off for that run. The agent-facing surface has no opt-out |
| `semlith add` refuses non-public addresses | a URL resolving, at any redirect hop, to loopback, RFC 1918, link-local (including `169.254.169.254`), carrier-grade NAT, a unique local address or the unspecified address exits non-zero naming the address | `SEMLITH_ADD_ALLOW_PRIVATE=1` |
| The session token is a header; the cookie is gone | a `Cookie: semlith_token=…` header or a `?token=…` query string gets 401; a browser holding a 0.13.0 cookie reloads from the printed URL | send `Semlith-Token: <token>`. A non-GET also needs a JSON content type and, if it sends fetch metadata, `Sec-Fetch-Site: same-origin`; a client sending neither `Sec-Fetch-Site` nor `Origin` is judged on the token alone, so `curl` works with one header changed |

The page and its static assets are now served without a credential (a browser
attaches no header to a stylesheet, font or favicon); everything about the
machine still needs the token. Also:

- The Linux archive holds `libonnxruntime.so` beside the binary rather than
  linking ONNX Runtime in, so it starts on Debian 12, Ubuntu 22.04 LTS, RHEL 9
  and Amazon Linux 2023 (issue #57). `install.sh` and `semlith upgrade` place
  both; a binary copied out alone names the missing library. `cargo install`,
  macOS and Windows are unchanged.
- A rotated agent key expires fifteen minutes later (`http::KEY_GRACE`), not
  when the daemon exits.

### 0.15.0

| Change | Detail |
|---|---|
| **Break:** `semlith start --ledger` removed | It fails at parse time. The ledger now records by default, and a no-op flag would read as switching something on. Delete it; to not record, use `--no-ledger` (a session) or `SEMLITH_LEDGER=0` (a machine), both read at write time |
| The ledger records by default | Every retrieval over stdio, `/mcp`, the CLI and the portal, graph tools included, writes a `retrievals` row. Through 0.14.0 only the portal's search box wrote one, and only with `--ledger` |
| `semlith_search` answers with locations | `format: locate \| excerpt`, default `locate` over MCP: store-relative path, line span, enclosing symbol and kind, the lists that found it, a freshness flag and one line of text, grouped by file and cut to `max_tokens` (default 1500, floor 200) with `truncated: N of M`. `format: "excerpt"` returns what 0.14.0 did. `/api/search` takes `format` but defaults to text. The CLI and `--json` still carry chunk text |
| Added | `semlith_languages`; on a hit `fresh` (always), `symbol`, `symbol_kind` and `provenance` (`provenance` is never sent from 0.36.0); on an edge `definitions`, `from_path`, `from_line`; the columns above |
| Two more confidence values | Edge confidence is `extracted`, `resolved`, `inferred` or `ambiguous`. The new two are computed at query time and never stored, but a consumer matching exactly two strings sees two it does not know |

### 0.16.0

- **Added:** `read <target>` (`path:start-end`, `path:line` or a symbol name),
  `pattern <query> --lang <name>`, `semlith_read`, `semlith_pattern`;
  `search --prefer code|docs|any`, default `any`. `/api/read` and `/api/pattern`.
- **Break:** MCP array arguments (`path`, `ext`, `lang`, `store`) declared
  `{"type": "array"}` without `items`, to fit two new tools in the same
  `tools/list` budget. Behaviour was unchanged; `items` returned in 0.33.0.
- **Break:** `semlith_symbol` returns a block with the definition, the resolved
  callers and callees and the ring two hops out, not a flat list.
  `/api/symbol` keeps `symbols` and adds `callers`, `callees` and `ego`.
- **Break:** the portal's Languages page is gone; its content is on About, and a
  bookmark to it lands there. `semlith languages`, `semlith_languages` and
  `/api/languages` are unchanged.

### 0.17.0

- The code graph covers all forty-six languages `--lang` accepts (it covered six):
  Ruby, PHP, Kotlin, Swift, Scala, Haskell, Lua, Elixir, Zig, Dart, YAML, TOML,
  JSON, Markdown, Terraform, Dockerfiles, Makefiles, GraphQL, protobuf, SQL, CSS
  and HTML among them. No surface or format changes; symbols appear when those
  files are next indexed.
- `semlith languages` prints a `graph` marker column between the name and the
  extensions. Parsing by the first field is unaffected.
- `THIRD-PARTY-NOTICES` ships in the crate and every release archive. The binary
  is larger by forty C parsers ([performance.md](performance.md#the-binary-and-what-a-linux-machine-needs)).

### 0.17.1

Five answers changed, each one that had reported success for an operation that
failed. No format change; 0.17.0 and 0.17.1 stores are byte-compatible.

| Change | Detail |
|---|---|
| `semlith index` on an unreadable path exits non-zero | Roots are checked before a store is chosen. All unreadable: nothing is created or registered. Mixed: readable roots are indexed and recorded, each unreadable one is named on stderr, and it still exits 1. `POST /api/index` answers 400 and `semlith_index` refuses on the same condition |
| `semlith forget` of an unindexed path exits 1 | It is anchored on the path, not the working directory, and prints `nothing to forget: <path> is not indexed` |
| `POST /api/forget` | 404 where it answered 200 with a zero count |
| `GET /api/search` | applies the `offset` it validated, and echoes it back |
| Printing into a closed pipe | exits 0 and prints nothing (`semlith files \| head` panicked) |
| Windows store home | `%USERPROFILE%\.semlith`. `user_home()` reads `HOME`, then `USERPROFILE`, then `HOMEDRIVE` plus `HOMEPATH`, and errors naming `SEMLITH_HOME` when none is set; `SEMLITH_HOME` is still read first. A `.semlith` created earlier in some working directory is not found or moved: copy it, delete it, or re-index |
| The directory deny-list fails closed | A path that cannot be checked against `~/.ssh`, `~/.kube` and the rest is refused (on Windows those rules had never run) |
| Paths print in the form the platform opens | The `\\?\` prefix (and `\\?\UNC\`, back to `\\`) is stripped from search hits, file, image and symbol rows and call-site paths, on the CLI, `--json`, MCP and `/api/search` and `/api/files`. The store keeps the verbatim form for paths over 260 characters; `semlith read` accepts either |

### 0.17.2

No user-facing surface or format change. The client stanzas moved from
`README.md` to `docs/clients.md` (same bytes, same heading levels, which
`src/clients.rs` parses); a fork that patched the README's client section moves
the patch. `GET /api/agents` returns the same clients in the same three groups.
The portal's Graph page draws `contains` and `aliases` edges.

### 0.18.0

- **`semlith setup` registers every client that documents a registration
  command**, at the scope that client spells "every project", and `--yes` does
  too. It runs a client's remove verb first at every scope, including Claude
  Code's `local` scope (an entry under `projects."…".mcpServers` in
  `~/.claude.json` hid semlith from every other directory). `--register-all`
  writes user-level files, listing every path and asking first. OpenCode is
  registered by its file, because `opencode mcp add` on 1.18.11 has no
  every-project flag.
- **Every registration is the stdio form.** No file semlith writes carries the
  agent key or names `${SEMLITH_AGENT_KEY}`; `semlith mcp` reads
  `~/.semlith/agent.key` itself, so `semlith key rotate` re-registers nothing.
  The shell startup block that exported `SEMLITH_AGENT_KEY` is gone: `setup`
  writes `PATH` only and replaces an older block. Open a new shell after running
  it. `SEMLITH_AGENT_KEY` stays covered for the HTTP stanzas, which you export
  yourself.
- **Added:** `doctor`, with `--json` and `--fix` (per-client install and
  registration state, plus the four Privacy rules that read this machine);
  `setup --register-all`. Routes: `POST /api/privacy/fix`,
  `POST /api/agents/register`, `GET /api/doctor`. A Privacy-page repair narrows
  access, is idempotent, touches only paths semlith owns and is confirmed by
  re-running the rule's check; `private addresses` has no button.
- `GET /api/setup`'s `claude_registered` is replaced by `registered_clients`,
  read from client files on disk.
- **An index is a function of its corpus.** The durability checkpoint counts
  files, not seconds, so `SEMLITH_CHECKPOINT_SECS` became
  `SEMLITH_CHECKPOINT_FILES` (neither covered). A 0.18.0 store is not
  byte-identical to a 0.17.3 one from the same corpus; two 0.18.0 builds are.
  Nothing needs re-indexing.

### 0.19.0

No format change and no ranking change.

- **The hidden-file rule applies only to a path you named.** A dotfile the walk
  yields (after your `.gitignore` whitelisted it) is indexed; one you name is
  still refused, as is the same path via `semlith_index` or `semlith add`. A
  re-index may therefore add files 0.18.0 left out. The credential rules apply
  either way.
- **The credential deny-list is wider:** `.env*` (replacing `.env` and `.env.*`),
  `*.env`, `*.env.*`, and `.npmrc`, `.netrc`, `.pypirc`, `.pgpass`,
  `.htpasswd`, `.boto`, `.s3cfg`, `*.ppk`.
- **A content scan refuses a file holding a credential**, before it is chunked
  or embedded. `--include-secrets` indexes it anyway and reports how many files
  the scan would have refused. See [security.md](security.md).
- **A refused file an earlier run indexed is evicted** in the same pass.
- **Added:** `scan [STORE]`, with `--forget` and `--json`; deliberately no MCP
  tool. `pattern` and `semlith_pattern` gain `path` (repeatable glob) and
  `offset`, both optional; the truncation line names the offset that continues
  it, and two calls with one offset return the same matches.
- **A per-file failure no longer ends a run.** A new outcome, `failed`, carries
  the error's message, and the run continues. The `done` event, the CLI summary
  and `semlith_index` name every failed path. An embedding batch failure is
  still fatal. The watcher survives a per-file failure and reports it on the
  store's feed.
- **Events say why.** `IndexProgress` and the daemon's `file` event carry `why`
  for `skipped`, `refused` and `failed`. Skip reasons are a closed set: `empty`,
  `over 8 MiB`, `not a regular file`, `unreadable: <the operating system's own
  message>`, `binary`, `no text in this document` and `not a decodable image`.
  `done` gains `failed` (`[{path, why}]`), `skipped_reasons` (`{"<kind>":
  <count>}`) and `elapsed_ms`.
- **A running daemon opens stores another process created**, on every read of
  `/api/stores` and every by-name miss. A store whose lock another process holds
  is listed as being written; one missing on disk as missing; such rows carry
  `unopened`.
- `semlith_symbol`, `semlith_path`, the `file` event and `done`'s paths print
  plain on Windows, as other surfaces have since 0.17.1.

### 0.20.0

No CLI, MCP or format change.

- **`POST /api/index` answers immediately instead of streaming NDJSON.** A script
  following the stream gets one object and polls `GET /api/index/runs` instead;
  a route pinning one of eight workers for a whole run stalled the portal. A
  forwarded `semlith_index` still streams. `POST /api/add` answers the same way,
  adding `fetched` and `url`; its fetch is still synchronous.

```json
{"runs": [{"run": 4, "store": "api", "path": "/Users/you/work/api"}], "target": "each"}
```

`target` is `"each"` or `"store"`. A run that could not be queued has `error` in
place of `run`. On the `store` target a row has `run` and `store` and no `path`.

| Route | What it answers |
|---|---|
| `GET /api/index/runs` | Every store's run with its queue position, the daemon-wide queue in submission order, how many runs are on, and the three settings with their source, the derived value, the sentence that derived it and the machine reading behind it. The machine is re-read per call, so the memory figure is memory free *now*. |
| `GET /api/index/log?store=<name>&after=<seq>` | That run's log lines after a cursor. A cursor rather than an offset, so two clients reading the same run through their own cursors each see every line exactly once. The daemon keeps the last 500. |
| `GET /api/projects?path=<dir>` | The git repositories directly under a directory — a `.git` directory *or* file, so a worktree and a submodule count — each with its name, canonical path and the store already covering it if any. Where none of the children is a repository, its plain subdirectories instead, with `repositories: false`. One level only. Confined to the user's home exactly as `/api/dirs` is. |
| `GET /api/changes` | Six monotonic integers, one per data domain: `stores`, `runs`, `clients`, `ledger`, `events`, `privacy`. The portal polls this once a second and refetches only what moved. It is not a server-sent stream because a stream would hold one of the eight workers per open tab. |
| `POST /api/index/settings` | Saves any of `runs_at_once`, `embed_threads` and `index_memory_mb`. A setting the environment fixes is refused with 409, naming the variable. |

- `POST /api/index` takes `"store": "each"`, one store per path, resolved as
  `semlith index <path>` resolves (including the numeric suffix for a second
  `api`). No store means `each` for several paths and one store for one path.
  `POST /api/index/control` gains `dequeue`; `pause`, `resume` and `stop` are
  unchanged.
- **Added:** `index --each` (one store per path, sequentially; refused with
  `--name`) and `index --projects <FOLDER>` (the repositories directly under the
  folder, or its plain subfolders if none is a repository; implies `--each`).
  Without either, several paths still go into one store.
- **Three indexing settings** are derived from the machine (logical cores, total
  memory, memory free now) unless set. Precedence: the environment, then what
  the portal saved in `~/.semlith/settings.json`, then the derivation.
  `semlith start` prints all three with their source. A large machine now uses
  more threads and memory than 0.19.0 did, a small one fewer.

| Setting | Variable | Field in `settings.json` |
|---|---|---|
| Runs at once | `SEMLITH_INDEX_PARALLEL` (new) | `runs_at_once` |
| Embedder threads per writer | `SEMLITH_EMBED_THREADS` | `embed_threads` |
| Index memory per store | `SEMLITH_INDEX_MEMORY` | `index_memory_mb` |

- **Runs wait for each other.** A run is admitted while fewer than runs-at-once
  are running, otherwise it waits in one daemon-wide FIFO. A daemon that exits
  with queued runs drops them, and its next start names each on the store's
  feed as never started, with any run that was going and what it committed.
- **The run clock measures the run**, from submission across every 45-second
  slice, stopped while held, frozen at the end.

### 0.22.0

Format 3 (above). Search hits gain a `definition` badge in `lists`: a chunk that
defines the exact name an identifier-shaped query typed is lifted above the
fused order, not scored into it, so it carries no weight and may carry
`definition` alone.

### 0.24.0

- **Added:** `hook` (above); `setup --no-hooks` and `--strict`; the `!` exclusion
  prefix; `SEMLITH_DEFAULT_IGNORES`; `initialize` returns `instructions`; the
  `raw-read` ledger row.
- A built-in table of generated and vendored directories (`node_modules` and its
  kind) is not indexed unless `SEMLITH_DEFAULT_IGNORES` turns it off.

### 0.25.0

Format 4 (above).

### 0.26.0

- **Added:** `impact`, `trace` and `report` on the CLI; `semlith_impact`,
  `semlith_trace` and `semlith_report` (thirteen tools to sixteen);
  `GET /api/impact`, `/api/trace`, `/api/map`, `/api/report` and
  `/api/ledger/replay`. `semlith_impact` restores the 0.12.0 tool with the
  current answer shape. The `tools/list` gate moved from 1 120 tokens to 1 600.
- **Break:** `POST /api/index` holds to the boundary it documents (issue #120).
  A request naming a `store` that does not contain the path gets 403 with the
  rule; a request with no `store` indexes into the store `home::resolve` picks
  for the path, which may be new. Post no `store`, or add the folder first with
  `/api/root`.

### 0.27.0

- **Reports.** Formats are `markdown`, `csv`, `json`, `html` and `pdf`.
  `GET /api/report?format=<one of the first four>` answers with the JSON
  envelope carrying the rendering as `text`; `format=pdf` answers with the PDF
  itself (`Content-Type: application/pdf`), and `semlith report --format pdf`
  needs `--out`. `/api/report` and `semlith report` take `window` (`all`, `day`,
  `week`, `month`, `quarter`) and `scope` (a repeatable store label); without
  them a report covers every open store over its own span (seven days for the
  change brief, everything else in full). An unknown `window` is 400 naming the
  five; an unknown `scope` is 400 naming the open stores. `store` is read as an
  alias of `scope` (it used to be ignored).
- **Every report states its window.** The line under the title reads "Generated
  <when> on this machine, over <stores>, covering <window>. Nothing left it.",
  and the JSON gains `window` beside `stores`. Undated reports (savings, health,
  gaps) say so. A script parsing that line verbatim must be updated.
- `~/.semlith/schedules.json` appears once a schedule is added; a 0.26.x binary
  ignores it.
- `semlith models` and `/api/models` lose their portal view (the About page's
  forty-eight-row table). `/api/pattern` gained one in 0.35.0, as Search's fourth
  mode.

### 0.28.0

No format change beyond the `variants` row. A 0.27.x binary opens a 0.28.0
store; its int8 queries search any fp16 vectors in it.

- **Added:** `accel [status | on <lane> | off <lane> | remove <lane>]`, with
  `--json`, over `cpu`, `gpu` and `cuda` (`status` is the default). It refuses,
  non-zero, the CPU off with no usable GPU lane, CUDA where unsupported, and any
  switch while `SEMLITH_ACCEL` is set. `doctor --gpu`, with `--json`, conflicts
  with `--fix` and `--brief`, downloads a lane's components first, and prints one
  row per lane: device, variant, the lowest cosine of the 32 fixture chunks
  against CPU fp32, and chunks/s; `n/a` with a reason for a lane with nothing to
  run on. `--json` rows have `lane`, `device`, `variant`, `cosine`,
  `chunks_per_s`, `passed` and `reason`.
- `SEMLITH_ACCEL` and `SEMLITH_SESSION_IDLE_SECS` (above). A missing
  `accelerators` field means CPU on, GPU on, CUDA off; a 0.27.x binary ignores
  it.

| Setting | Variable | Field in `settings.json` |
|---|---|---|
| Which lanes embed | `SEMLITH_ACCEL` (new) | `accelerators`, an object with the optional booleans `cpu`, `gpu` and `cuda` |

- **Saved limits are applied.** `embed_threads` and `index_memory_mb`, saved but
  ignored in 0.27.0, now take effect (threads at each writer's next batch,
  memory on every open index at once). A saved `embed_threads` of 1 limits every
  writer to one thread. `POST /api/index/settings` replies with `applied`.
- **Watcher catch-ups and event batches of more than 32 files are admitted like
  runs** and appear in `/api/index/runs` with a `kind` other than `run`.
- `semlith stats` prints `variants 1904 int8-cpu, 2210 fp16-webgpu` when the row
  exists; `semlith_stats` prints it per store plus one `embedding lanes on:`
  line.
- **The login service definition is rewritten** by `setup` and `upgrade`: the
  macOS plist's `ProcessType` goes from `Background` to `Standard`, and the
  Windows logon task gets `-Priority 5`; the Linux unit is unchanged.
  `semlith doctor` prints `FAIL service priority` for an unrewritten definition.
  An older binary under a rewritten plist runs at normal priority throughout;
  its own `setup` restores its plist.

| Route | What changes |
|---|---|
| `GET /api/accel` (new) | `lanes`: one object per lane with `lane`, `enabled`, `status` (an object whose `state` is `idle`, `starting`, `active`, `downloading` with a `percent`, or `unavailable` or `failed` with a `reason`), `device`, `variant`, `rate` in chunks/s over the last 10 s, and `share` as a percentage of the total rate. Also `source` (`default`, `saved` or `set by the environment`), `cpu_fallback` (true when the CPU is carrying the work although its switch is off), and `bytes`: what the `gpu` and `cuda` components take on disk, plus `cuda_download`, the size of the pack. |
| `POST /api/accel` (new) | `{"lane": "cpu" \| "gpu" \| "cuda", "action": "on" \| "off" \| "remove"}`. Answers with the same body as the GET, plus `said`. A refusal is 409 with the reason. |
| `POST /api/doctor/gpu` (new) | No body. Runs `semlith doctor --gpu`'s check on the daemon, and may download a lane's components first: `checks`, one object per lane with `lane`, `device`, `variant`, `cosine` (the minimum over the 32 fixture chunks), `chunks_per_s` and `passed`, or `lane`, `passed: false` and `reason` when the lane cannot run or is off. |
| `GET /api/index/runs`, per run | `kind`: `run`, `catch-up` or `batch`. `files_before` and `chunks_before`: what the store held when the run started. `threads`: the intra-op thread count the run's session was built with, null until it has one. `rate`: chunks/s over the last 10 s of active time, null before the first batch and once the run has finished. `rate_average`: chunks/s since the first batch. `lane_rates`: chunks/s per lane, for example `{"cpu": 25.0, "gpu": 48.1}`. `delete`: null, or the sentence saying what became of the store when a stop was confirmed with the delete box ticked. `status` gains `pausing` and `held`. |
| `GET /api/index/runs`, top level | `held`: the ids of runs held by a lowered *runs at once*, oldest first. A run whose stop deleted its store stays in the list after the live stores' runs until it is removed. |
| `POST /api/index/control` | `stop` takes `"delete": true`, which removes the store after the undo, the same way `semlith drop` does. The reply carries `state` (`pausing`, `running` or `stopping`) and `deleting`. `pause` and `resume` answer with the requested state at once. The engine reaches that state at its next batch. |
| `GET /api/about` | `priority`: `managed`, `state` (`background` or `normal`), `embedding` (the number of embed passes in progress), `switches` and `last_switch_us`. `sessions`: `writers`, the writer embedding sessions loaded, and `query`, the shared query sessions loaded. |
| `GET /api/stores`, per store | `stopped_because`: why the store's writer ended, beside `watching: false`. |
| `GET /api/privacy` | `downloads`: every download this binary can make, each with `what`, `source`, `bytes`, `when` and `cached`. |

`held` and `pausing` are new status values.

### 0.29.0

No covered change; stores and `settings.json` are untouched.

| Route | What changes |
|---|---|
| `GET /api/index/runs`, per run | `bytes` and `bytes_total`: the bytes of the files the run will open, done and in all, with a file being embedded counted in proportion to its chunks. `eta_ms`: milliseconds left at the bytes/s rate over the last 10 s of active time, null until 5 s of embedding has been seen and whenever the run is not `running`. `started_at` and `finished_at`: unix seconds, beside the `submitted` that was already there. `queued_ms`: milliseconds on the run's clock before it started. |
| `POST /api/store/delete` | Takes `{"stores": ["a", "b"]}` as well as `{"store": "a"}`. Each store in the list goes through the same removal as a single one, and a store that cannot be deleted does not stop the others. The list form answers `{deleted: [...], failed: [{store, error}], message}`: 200 when any store was deleted, and 409 with an `error` naming why when none was. The single form and its answer are unchanged. |

### 0.30.0

Additive apart from two answer-shape changes (marked **Changed**). The first pass
rescans every file under the new secret rules.

| Surface | What changes |
|---|---|
| `semlith refused` | New. Lists every not-indexed file with its class and rule; `accept <path> --redact\|--as-is [--yes]`, `revoke <path>` and `refuse <path>` act on one path per call. |
| `semlith index` | New `--scan-only` (the plan, no embedding) and `--no-review`. On a terminal, a run stops once per reviewable file before embedding; piped, with `--no-review`, or from MCP, it never asks. |
| `semlith files` | New `--tree`, `--depth`, `--sort name\|size\|symbols\|recent`, `--path`. Without `--tree` the listing is unchanged. |
| `semlith symbol` | Takes several names; with more than one it prints one row per definition and no rings. |
| `semlith setup` | New `--hook-mode soft\|gate\|hard` (`--strict` is `gate`) and `--no-agents`. Writes `alwaysLoad` on Claude Code's user-scope entry, the hook matcher `Bash\|Read\|Grep\|Glob` plus a `PostToolUse` entry, and `~/.claude/agents/semlith-explorer.md`. `--no-hooks` removes both hook entries. |
| `semlith hook` | New `--mode`; `--strict` is kept as `--mode gate`. Reads `Bash`, `Glob` and `PostToolUse` events. A bounded `Read` of an indexed file is now nudged too. |
| `semlith doctor` | Reports `alwaysLoad`, the hook mode and the research agent. `--fix` also clears a per-project disable of semlith for the current directory in `~/.claude.json`. |
| `semlith_symbol` | `name` is no longer required when `names` (up to 20) is given. |
| `semlith_files` | New `tree`, `depth`, `sort`. |
| `semlith_impact`, `semlith_neighbors`, `semlith_symbol` | Accept `Type::method`, `module::function` and `Type.method`. Answers are capped at 16 000 characters. Impact rows carry the call-site line. |
| `semlith_search` (locate), `semlith_read`, impact, neighbors, symbol, files | **Changed:** paths are relative to the store root, which a `root …` line names once at the top, instead of absolute. `semlith_read` resolves such a path against the store's roots. A locate row is one line: `start-end name kind @line · lists | best line`. |
| `semlith_read` | A name with up to four definitions returns each whole, capped at 32 000 characters; a file edited since indexing is read from disk (scanned, and redacted if accepted that way) and marked. |
| `semlith_stats` | Languages with fewer than five files collapse into one line. `semlith stats` is unchanged. |
| MCP `instructions` | Built from the indexed folders, at most 600 bytes; it no longer tells the agent to call `semlith_stats` first. |
| `symbols.qualified` | **Changed** for Rust methods written by 0.30.0: `Type::name` (the `impl` or `trait` owner) instead of `module::name`. Rows written by an older binary keep theirs until the file is re-indexed. |
| Secret scan | A match that is a declared test dummy no longer refuses its file; a secret-sounding name assigned a random literal, quoted or not, now does. The first pass under 0.30.0 rescans every file. See `docs/security.md`. |
| `GET /api/refused`, `POST /api/refused/accept`, `POST /api/refused/revoke` | New. The two writes take one `path`, refuse `paths`, and need the session token. |
| `POST /api/index` | New `review`: `"always"` holds every run after its scan, even when nothing needs a person (the portal's **Scan**); `true` holds a run only when something is reviewable. New `scan_only` (answers `{plan}` per path, starts nothing), which the portal no longer sends. |
| `GET /api/index/runs`, per run | New `plan`, and a `review` status for a run held after its scan. |
| `POST /api/index/control` | `start` queues a held run; `stop` drops one, and with `delete: true` also deletes its store once dropped (the portal sends it only for a store that held no files before the scan). |
| `GET /api/files` | `tree=1` answers `{tree}` with the text the MCP tool gives. `tree=1&format=json` answers one level instead: `dir` is a folder relative to a root (absolute or climbing with `..` is refused), `store` and `sort` as for the text form, and the answer is `{roots: [{store, root, dir, dirs: [{name, files, chunks, langs}], files: [{name, lines, symbols, chunks, stale, lang}], not_indexed: [{name, why, dir}], more}]}`, one entry per store root that holds `dir`, at most 1 000 entries a level with the rest counted in `more`. |

### 0.31.0

A compacted store is an ordinary store any binary from 0.23.0 on reads: only
rows whose chunk no longer exists, freed pages and history past the retention
are removed, and shards keep their layout and naming. History dropped by
retention does not come back on an older binary.

| Surface | What changes |
|---|---|
| `semlith compact` | New. `--all`, `--retention DAYS` (0 keeps all history), `--dry-run`, `--json`. |
| `semlith stats` | New `disk` line: total, live, reclaimable, reclaimable share, and `re-index to compact vectors` for a store that cannot have its vectors compacted. The `files` and `chunks` lines are unchanged. |
| `semlith_stats` | The first line per store gains `, N on disk (M reclaimable)`. |
| `POST /api/store/compact` | New: `{store, retention_days?, dry_run?, wait?}`. Without `wait` it answers `{store, run, message}` at once; with it, `{store, run, stopped, compact}`. |
| `GET /api/stores`, per store | New `disk`: `{total, live, reclaimable, dead_percent, database, exact, vectors, compacts_vectors}`. `bytes` is still the indexed source. |
| `GET /api/index/runs`, per run | A run's `kind` may be `compact`; its summary carries `compact`, the report. The answer gains `compaction`: `{threshold_percent, retention_days, default_threshold_percent, default_retention_days}`. |
| `POST /api/index/settings` | Takes `compact_threshold_percent` (0-99, 0 is off) and `history_retention_days` (0-36500, 0 keeps all); the answer carries `compaction`. |
| `settings.json` | Two optional fields, `compact_threshold_percent` and `history_retention_days`. A missing field means the default, 25 and 90; an older binary ignores both. |
| The ledger | A search whose text holds a live-shaped key records it masked (`[REDACTED:…]`). Rows already written are unchanged. |
| `SEMLITH_DOWNLOAD_STALL` | New: seconds a model download may receive nothing before it fails, default 60. |
| Store meta | `vectors_swapping` is written during a compaction's swap and left `0`. |

### 0.32.0

A 0.32.0 store is readable by any binary from 0.23.0 on. The vector cache
(`cache/vectors.db` under the semlith home) is never opened by an older binary
and can be deleted.

| Surface | What changes |
|---|---|
| `semlith accel` | The lanes are `cpu`, `gpu`, `ane` (the Neural Engine), `cuda`, `trt` (TensorRT for RTX), `openvino` and `llama` (llama.cpp), plus the switch `gpu-beside-ane`. `on` for a lane whose pack is not installed fetches it first. `status` names the state (now also `compiling` with a percent) and says `(experimental)` beside CUDA, TensorRT for RTX, OpenVINO and llama.cpp, and prints the vector cache line. |
| `semlith doctor --gpu` | One row per lane: device, known-answer cosine, the reason a lane is unavailable, the fallback, and `(experimental)` where it applies. A failed check prints `FAIL` rather than `n/a`. |
| `semlith index` | `--verbose` (`-v`, new) prints where the run's time went, what each lane embedded, each lane's state at the end, and the vector cache's hits. `index` and `watch` use the accelerator lanes, as the daemon does; `SEMLITH_ACCEL=cpu` keeps them on the CPU. Files changed most recently are embedded first. |
| `semlith search` | While a store is still being embedded, one line on stderr says how much of it is pending. |
| `semlith stats` | New `cache` line: vectors, size, cap, and the share of lookups that hit. |
| `semlith setup` | On Apple silicon a new step fetches the Neural Engine pack. |
| `semlith_stats` | `embedding lanes on:` names the new lanes, experimental ones marked; a new last line `vector cache: …`. |
| `semlith_search` | Mid-run, a last line saying how much of each store is still being embedded. |
| `GET /api/accel` | Each lane gains `label`, `experimental`, `installed` and `download_bytes`; `status.state` may be `compiling` or `downloading` with a `percent` and, once it can be told, `eta_ms`; the body gains `gpu_beside_ane`; `bytes` gains `ane`, `trt`, `openvino`, `llama` and their `_download` sizes. |
| `POST /api/accel` | Takes the new lanes and `gpu-beside-ane`. `on` for a lane whose pack is not installed answers at once and fetches it in the background; the lane's status shows the download. |
| `GET /api/index/runs`, per run | New `stages` (`wall_ms`, `walk_ms`, `read_ms`, `extract_ms`, `parse_ms`, `tokenize_ms`, `write_ms`, `embed_wait_ms` per lane, `prepare_cpu_ms` per part), `cache_lookups`, `cache_hits`, `cache_hit_rate`, `pending_share`, and `rows`. `lane_rates` may name `ane`, `trt`, `openvino`, `llama` and `cache`. |
| `GET/POST /api/index/settings` | New `vector_cache`: `{cap_mb, default_cap_mb, from_environment, vectors, bytes, hits, lookups}`; POST takes `vector_cache_mb` (0 is off, at most 65 536). |
| `GET /api/search` | New `pending`: `[{store, share}]`, absent when nothing is being embedded. |
| `GET /api/about` | `priority` gains `indexing_threads` and `indexing_class`. |
| `settings.json` | New optional fields: `accelerators` gains `ane`, `trt`, `openvino`, `llama` and `gpu_beside_ane`; `gpu_adapter`; `vector_cache_mb`. A missing field means the default: the Neural Engine on, the experimental lanes off, the GPU off beside the Neural Engine, a 1 024 MB cache. A saved `cuda: true` stays on. An older binary ignores them all. |
| The model cache | New under `accel/`: `coreml-worker-v1`, a copy of semlith (a hard link where the file system allows) that the Core ML lanes run from so macOS's compile cache outlives upgrades; `coreml-compile.lock`; and `<lane>.compile-ms`, how long the lane's last compile took. All three can be deleted; the next start makes them again, compiling once more. |
| Environment | New: `SEMLITH_COREML_WORKER` (`current` runs the Core ML lanes from the running binary), `SEMLITH_VECTOR_CACHE_MB`, `SEMLITH_GPU_ADAPTER`, `SEMLITH_OPENVINO_DEVICE`, `SEMLITH_LLAMA_DEVICE`. `SEMLITH_ACCEL` takes the new lane names. |
| The login service | On Linux the systemd unit gains `Nice=5`, `CPUWeight=50` and `IOWeight=50`; a unit without them is reported stale and rewritten by `semlith setup`. On macOS an index run's threads run at Utility QoS; on Windows the daemon stays below normal priority, with EcoQoS off while it embeds. |

Which lane runs where — each claim is exactly what was measured:

| Lane | macOS (Apple silicon) | Linux x86_64 | Windows x86_64 | Measured |
|---|---|---|---|---|
| CPU (int8) | yes | yes | yes | M1: 30.7 chunks/s lane alone, 35.5 with spinning off |
| Neural Engine (`ane`) | yes, with the pack | — | — | M1: 236.5 chunks/s lane alone, sustained; the release gate is end to end |
| GPU through Core ML | yes, with the pack | — | — | M1: 73.2 chunks/s lane alone |
| GPU through WebGPU | without the pack | Vulkan | D3D12 | M1: 43.7 chunks/s lane alone; not measured on Linux or Windows |
| CUDA (experimental) | — | yes | — | built and checked without hardware |
| TensorRT for RTX (experimental) | — | yes | yes | built and checked without hardware |
| OpenVINO (experimental) | — | yes, Intel hardware | yes, Intel hardware | built and checked without hardware; known answer on its CPU device in CI on an Intel runner. Intel's plugin offers Intel devices only: on an AMD CPU with no Intel GPU the lane says so and the run goes on without it |
| llama.cpp (experimental) | Metal | Vulkan | Vulkan | M1 Metal known answer at cosine 0.9999995; not measured for throughput in this release |

### 0.33.0

The agent clients are these twelve, in three groups: Claude Code, OpenAI Codex,
OpenCode, IO CLI, GitHub Copilot CLI and Gemini CLI (Terminal); GitHub Copilot in
VS Code, Cursor, Zed and Cline (Editors); Claude Desktop and ChatGPT desktop (the
Codex app) (Desktop apps). `GET /api/agents`, `semlith doctor` and its `--json`
report exactly these, each with its own registration CLI or a user-level file
`semlith setup --register-all` writes. Zed and Cline are registered by file:
`~/.config/zed/settings.json` and `~/.cline/data/settings/cline_mcp_settings.json`
(under `CLINE_DIR` when set). A Zed file with comments or trailing commas is left
untouched and the stanza printed with the reason.

| Surface | Change |
|---|---|
| MCP `tools/list` | Every array parameter carries `"items": {"type": "string"}`. The values accepted are unchanged: they were always strings. |
| `semlith setup` | A new step, `leftovers`, prints what it removed from clients' files; the `service` step restarts a login-service daemon serving another version and says so. Printed stanzas name `${SEMLITH_AGENT_KEY}`. |
| `semlith doctor` | A `daemon version` row, which fails while the running daemon serves another version; the Gemini CLI row may carry a folder-trust note. `--json` gains `daemon`: `{version, binary, started, stale}`. `--brief` exits non-zero on a stale daemon. |
| `GET /api/doctor` | No longer carries `unregisterable`. |
| `daemon.json` | New field `started`, the unix second the daemon began. A 0.32.0 binary ignores it; a 0.33.0 binary reads a file without it. |
| `docs/clients.md` fences | A `root=` attribute names an environment variable that, when set, replaces the path's first directory under `~`. `os=` takes a comma list (`macos,linux`), a `register`/`unregister` fence may carry it, and a path may start `%LOCALAPPDATA%\`. |
| `semlith setup --register-all` | Appends a `[mcp_servers.semlith]` table to Codex's `config.toml` when it has none; writes a client's file when its CLI is not on `PATH` but the client is installed; on Windows runs a client CLI's `.cmd` shim. |
| The model cache | The Core ML pack is `accel/coreml-2` and the worker copy `accel/coreml-worker-v2`; the first start after the upgrade downloads the pack (148 MB) and compiles its models once. `coreml-1` and `coreml-worker-v1` can be deleted. |
| The ledger | `client` holds the documented client name (`Claude Code`, `Zed`, `ChatGPT desktop (the Codex app)`, …) where the app can be told, rather than the raw `clientInfo.name`; an unrecognised name inside a known app is `name (app)`. A new nullable column `client_version`, outside the hash chain as `query_id` is, so a 0.32.0 binary still verifies every row. |
| `GET /api/agents` | `connections` is one entry per app and transport, with `sessions` and `version` (the versions seen, comma-separated); `queries` is summed across the sessions. |
| `semlith mcp` ↔ daemon | The proxy sends a `Semlith-Host` header naming the app that started it, a `notifications/semlith/alive` heartbeat every 30 s and `notifications/semlith/closed` when its client hangs up; `DELETE /mcp` ends an HTTP session. A failed call is retried for up to 20 s while the discovery file names a live daemon. |
| `semlith mcp` | The start line on stderr reads `semlith <version>: forwarding MCP to the semlith daemon at http://127.0.0.1:<port>`. |

### 0.35.0

The portal's v6 pages are drawn from these. Every field is additive except the
three marked as a shape change.

| Surface | Change |
|---|---|
| `registry.json` | Each store entry gains `kind` (`code`/`docs`/`both`, default `both`), `lean` (`code`/`docs`/`either`, default `either`), `watch`, `record` and `gitignore` (each default `true`). An older binary ignores them; an entry without them reads as the defaults, which are what every store did before. |
| `settings.json` | New keys `ledger_paused` and `airgap`, absent meaning off. `session_replay` absent now means **on**; a file that wrote `false` stays off. |
| A store directory | A new `runs.jsonl`: finished index runs, newest 50 kept. An older binary never opens it. |
| `POST /api/store/create` | New: `{name, kind}` → `{name, dir, kind}`; an empty store, served at once. |
| `POST /api/store/settings` | New: `{store, rename?, kind?, lean?, watch?, record?, gitignore?}` → `{name, kind, lean, watch, record, gitignore}`. A rename moves the store's directory with its name. |
| `GET /api/stores` | Rows gain `kind`, `lean`, `watch`, `record`, `gitignore`; with `?detail=1` (or `?coverage=1`) also `readers_count` and `languages_count`. |
| Searches and briefs | With no `prefer`, each store's `lean` applies to its half of the answer, over MCP as well as the portal. `either`, the default, is no bias, so nothing changes until a store sets one. |
| `GET /api/search` | `format=locate` hits carry `line`, the tool's one line; the answer carries `tokens`, and with `max_tokens` the tool's own cut and `truncated: {shown, total}`. |
| `GET /api/brief` | Gains `text` (what `semlith_brief` returns, byte for byte) and `prefer`. |
| `GET /api/ledger` | **Shape change:** `recording` is `{on, reason}` (`flag`, `env`, `paused` or null), not a boolean. Gains `break: {store, row, at}` when the chain does not verify. |
| `POST /api/ledger/recording`, `POST /api/ledger/verify` | New. Pause/resume (persisted); re-walk every chain, and with `repair` append one note row per break. A note row (`tool = 'note'`) is in the chain and in no total. |
| `GET /api/about` | Gains `recording` and `login: {installed, mechanism, path, last_start}`. `ledger` is the live state (false while paused). |
| `GET /api/privacy` | **Shape change:** `airgap` is `{on, reason}` (`flag`, `env`, `runtime` or null). Gains `outbound: {count, since, recent: [{at, what, host}]}`. |
| `POST /api/airgap`, `POST /api/login-item` | New. The runtime airgap refuses what `--airgap` refuses; the login item installs or removes the service `semlith setup` installs. |
| `GET /api/refused` | Rows gain `risk`, `band`, `likely`, `tone`, `kind`, `why`, `evidence`, `suggest`; scan-plan review items carry the same, and plans gain `not_indexed_paths`. |
| `POST /api/refused/decide`, `GET /api/decisions` | New: bulk `in`/`redact`/`out`/`reset`, and the decisions table. `/api/refused/accept` and `/revoke` also take `files`. Keep out is an acceptance with mode `refused`, now allowed for any reviewable class. |
| `GET /api/index/runs` | Gains `history`. Live run `kind` may also be `reindex`, `rebuild` or `files`. |
| `POST /api/index` | `{store, files}` re-indexes those files forced; `{store}` with no path re-indexes the store (`force: true` re-embeds unchanged files too); `gitignore: false` walks past `.gitignore` and is kept on the store. |
| `POST /api/agents/register` | Takes `{clients, action}` with `action` `register` or `unregister`. `GET /api/agents` client rows gain `id` and `registered`; `tools` rows gain `answers`, `typical_tokens` and `typical_source`. |
| `semlith start --no-ledger` | Now stops the rows written by agents through `/mcp` too, not only the portal's. |

### 0.36.0

| Surface | Change |
|---|---|
| Search ranking | **Behaviour change:** search no longer builds the graph list. No hit's `lists` holds `graph`, and the `provenance` field is never sent. `neighbors`, `impact`, `path`, `trace` and brief's callers and callees still read the graph. |
| `tools/list` | **Shape change:** eight tools are listed (`semlith_search`, `semlith_brief`, `semlith_read`, `semlith_files`, `semlith_symbol`, `semlith_neighbors`, `semlith_impact`, `semlith_stats`) and their schemas declare fewer arguments. The server still answers every tool and argument, but a client offers its agent only listed tools, so writes, reports, pattern, path, trace and languages leave agents unless `SEMLITH_MCP_TOOLS=all` is set in the server's environment; the CLI and portal keep them all. |
| `semlith_brief`, `semlith_symbol`, `semlith_neighbors`, `semlith_impact` | Take `path`, `ext` and `lang` (a leading `!` excludes), as search does, on MCP, the CLI (`--path/--ext/--lang`) and `/api/brief`, `/api/symbol`, `/api/neighbors`, `/api/impact`. Scoped, only definitions, callers, callees and reached rows in selected files are answered. |
| `GET /api/graph` | Answers 503 with a sentence naming how to scope it when the drawing takes longer than 15 s. |
| `GET /api/corpus` | A store over 20 000 chunks may answer `{store, measuring: true}` while it is measured off the request; the measure is kept until the store changes. |
| Release assets | Each release carries `install.sh` and `install.ps1`, and every asset has a GitHub artifact attestation. The installers verify the archive's with `gh attestation verify` when `gh` is installed and logged in, and refuse one that fails; a tag before `v0.36.0` has none and is installed as before. |

### 0.37.0

The Semlith Cloud client. Nothing below is reached by a machine that never ran
`semlith cloud login`.

| Surface | Change |
|---|---|
| CLI commands added | `cloud login [<org>] [--host] [--token]`, `logout`, `status [--json]`, `connect <org> [--store <name>…]`, `disconnect <org>`, `push <org>/<store> <dir> [--wait] [--json]`, `sync <store> on\|off [--org]`, `report <org> <kind> [--format] [--window] [--model] [--stores] [--out]`, `replay [<session>] [--org]`. Against the Semlith Cloud API's `/v1` routes. |
| `~/.semlith/cloud.json` | New, owner-only: `{machine, entries: [{host, org, token, plan?, added}]}`. Tool-written state, not a configuration file. A token is sent only to the `host` it is stored with. |
| `registry.json` | A top-level `remote` map, `<org>/<store>` → `{host, org, store, mcp_url}`, absent while empty. Not entries in `stores`: an older binary walks those as directories. An older binary ignores the map and drops it on its next write; `semlith cloud connect` puts it back. Store entries gain `cloud_sync: {org, since}`, absent while off. |
| The ledger | `retrievals` gains `synced_at`, additive and outside the chain; `FORMAT_VERSION` does not move. |
| MCP and the read routes | A search naming a remote store, or none while remote stores are connected, merges remote hits by score; any other tool naming one forwards it. Unchanged for a machine with no remote store. `/api/search` rows from a remote store carry `remote`, `badge`, `source`, `revision` and `behind_seconds`; other read routes add `remote: [{store, badge, text}]`; either may add `remote_skipped`. |
| Writes naming a remote store | Refused with a sentence on every path: the CLI, the write routes (400) and the MCP write tools. |
| Routes added | `GET /api/cloud`, `GET /api/cloud/status`, `POST /api/cloud/sync`, `/api/cloud/connect`, `/api/cloud/disconnect`, `/api/cloud/push`, `/api/cloud/report`, `/api/cloud/replay`. `/api/stores` gains `remote`, `/api/about` and `/api/privacy` gain `cloud`. |
| `semlith doctor` | A cloud line, and `cloud: {signed_in, orgs}` in `--json`. |
| Library | `Semlith::index_paths_under`, `index_rest_under` and `undo_run`; `mcp::Session` gains `host`. |

The `remote` lane and `semlith worker` are not in the binary from 0.37.0
(0.37.0-rc.5 to rc.7 had them): `semlith accel on remote` is refused as an
unknown lane, and `settings.json`'s `remote` object is ignored. In the library
the `remote` and `attest` modules and `ledger::measure` are gone; a program that
runs a worker of its own registers it with `accel::set_remote` (an
`accel::Remote` returning an `accel::RemoteChannel`), and only then does the
`remote` lane exist.

From 0.37.0-rc.6, the CPU cap and lane truth:

| Surface | Change |
|---|---|
| CLI | `accel off cpu` now always refuses: the CPU lane is always on. `accel on <lane>` refuses a lane this machine's hardware cannot run (no usable GPU for `gpu` and `llama`, no NVIDIA card and driver for `cuda` and `trt`, a non-Intel CPU for `openvino`) and a lane that failed. `accel status` and `doctor --gpu` print the CPU cap. |
| `settings.json` | `cpu_cap_percent` (0-100, absent = 100). A saved `accelerators.cpu: false` is read as on and cleared by the next save. An older binary ignores `cpu_cap_percent`. |
| Environment | `SEMLITH_CPU_CAP` (0-100) ahead of the saved setting. |
| `GET /api/accel` | Each lane row gains `saved` and `source` (`environment`, `saved`, `default`); the CPU row gains `locked` and `locked_reason` and its `status` is `active` only while it embeds. The body gains `cpu_cap` (`percent`, `source`, `measured_percent`, `paused`). |
| `POST /api/index/settings` | Takes `cpu_cap_percent`; outside 0-100 is a 400, and a value `SEMLITH_CPU_CAP` sets is a 409. The `limits` it and the runs route answer with gain `cpu_cap_percent` and `cpu_measured_percent`. |
| Library | `cpucap` module; `accel::CPU_ALWAYS_ON`; `embed::threads_in_force` never exceeds the cap's share of the cores. |
| Portal parity | The cap is a row on Settings › Performance › Limits; the CPU switch is locked there. |

From 0.37.0-rc.7:

| Surface | Change |
|---|---|
| Portal | The Limits card's CPU cap stepper moves in steps of 5 % instead of 10, snapping a value set off that grid onto it. No CLI, API, `settings.json` or library change. |

### 0.38.0

No covered surface changes and `FORMAT_VERSION` stays 4. A store gains an index
on the lowered file path when it is opened; older binaries ignore it.

## What a break looks like

If a covered surface has to change:

1. **It goes in the CHANGELOG**, in that version's section, stated as a break.
2. **The entry says why.** If the reason does not survive being written down,
   the change does not either.
3. **The entry says what to do**: the new spelling, the replacement flag, or the
   one-line edit to a configuration stanza. For a store format change, whether a
   re-index is needed and what it costs.

Before upgrading, read the CHANGELOG section for the version you are moving to.
