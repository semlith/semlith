# Compatibility

Which parts of semlith are a contract you can build against, which are free to
change, and what changed in each release. Read it if you wire semlith into a
script, an agent configuration or another crate, or before you upgrade.

## What is covered

These surfaces are the public contract. A change that breaks one is recorded as
a break.

| Surface | What is promised |
|---|---|
| CLI commands | The names and what each does: `index`, `start`, `key`, `adopt`, `trust`, `watch`, `search`, `brief`, `read`, `pattern`, `stats`, `files`, `forget`, `compact`, `refused`, `scan`, `drop`, `add`, `mcp`, `hook`, `setup`, `doctor`, `accel`, `upgrade`, `models`, `languages`, `ledger`, `prices`, `symbol`, `neighbors`, `impact`, `trace`, `report`, `schedule`, `cloud`, `path`. `__embed-worker` and `__parse-worker` are hidden and not part of the interface. |
| CLI flags | Flag names, short forms and meanings, including the repeatable `--store`/`-s` on read commands and the single `--store` write commands take. A leading `!` on a `--path`, `--ext` or `--lang` value excludes, applied after the inclusions of its kind; a value without `!` means what it always meant. The same holds for the `path`, `ext` and `lang` fields of every MCP tool that takes them. |
| Environment | `SEMLITH_STORE` (a path-separator-delimited list, split the way `PATH` is), `SEMLITH_HOME`, `SEMLITH_PORT`, `SEMLITH_AIRGAP`, `SEMLITH_EMBED_THREADS`, `SEMLITH_MCP_INDEX_BUDGET`, `SEMLITH_INDEX_MEMORY`, `SEMLITH_INDEX_PARALLEL` (runs at once), `SEMLITH_ADD_ALLOW_PRIVATE`, the `SEMLITH_AGENT_KEY` an HTTP stanza names, `SEMLITH_LEDGER` (`0`, `off` or `false` stops recording on this machine), `SEMLITH_DEFAULT_IGNORES` (`0`, `off` or `false` turns off the built-in table of generated and vendored directories), `SEMLITH_ACCEL` (a comma list of lanes such as `cpu,gpu`; a lane it omits is off, and it takes precedence over the saved switches) `SEMLITH_SESSION_IDLE_SECS` (how long a writer keeps an unused session, 60 by default) and `SEMLITH_PARSE_MEMORY_MB` (what one parse worker may allocate, 2048 by default). |
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
| The install scripts | `install.sh` and `install.ps1` stay at the root of the `main` branch, so the two `raw.githubusercontent.com` URLs in the README keep working. They honour `SEMLITH_VERSION`, `SEMLITH_HOME`, `SEMLITH_YES` and `SEMLITH_NO_SERVICE`, and verify `SHA256SUMS.sig` with `ssh-keygen -Y verify` and then the download against `SHA256SUMS` before writing anything. If `semlith.com` ever serves the installers, it redirects to these URLs rather than replacing them. |
| Release archives | One archive per target, named `semlith-<tag>-<target>`, holding a directory of that name with the binary in it, and a `SHA256SUMS` asset beside them in GNU `sha256sum` format. From v0.40.0 each also has a `.sig` beside it: an OpenSSH `SSHSIG` (namespace `file`) by the key `semlith --version` names. Linux archives also hold `libonnxruntime.so`, and every file in an archive is unpacked. `semlith upgrade` and both scripts read that layout. |
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
| 4 | code chunks embedded with the definition they sit inside | 0.25.0 through 0.38.1 |
| 5 | format 4 whose vector files may be turbovec v8 | 0.39.0 and later |

**A binary refuses a store whose format is newer than it knows**, naming both
numbers. Misreading a newer store does not look like an error; it looks like a
corpus that stopped containing things. One sharp edge: 0.5.0 and earlier predate
the key, so they read a format-2 store as an empty corpus instead of refusing it.
Do not point a pre-0.6.0 binary at a store 0.7.0 or later created.

**Forwards, every store still opens.** A newer binary reads, searches and
indexes into an older store without re-embedding and without changing its
format number, except that 0.39.0's first write moves format 4 to 5 (below).
A format-1 store is never migrated: its quantized vectors cannot
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

**Format 5 re-embeds nothing.** 0.39.0 moved to turbovec 1.1, which reads the
v7 vector files earlier releases wrote and writes v8 the next time it saves one.
A semlith before 0.39.0 cannot load v8, so the first write to a format-4 store
(an index pass, a `forget`, anything that saves) moves the row to 5 before
anything is saved; the vector files then follow shard by shard, each as it is
next written. Reading, searching and `stats` move nothing, so a store 0.39.0 has
only read still opens under 0.38.1. Once moved, an older binary refuses the
store with its usual "upgrade semlith" message rather than misreading a v8
file. Rolling back from format 5 means restoring the store from before the
upgrade or re-indexing under the older binary.

**Added tables and columns do not move the number.** The schema is applied with
`CREATE TABLE IF NOT EXISTS` (and nullable `ALTER TABLE` additions) on every
open, while `format_version` is written only when a store is created (and by the
re-chunking pass and the format-5 move above); bumping it
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

### 0.50.0

The CPU lane can be switched off again, beside another lane. `FORMAT_VERSION`
does not move and no store is re-indexed. No MCP schema, registry or
`settings.json` key changes. Rolled back to 0.49.0, a saved `accelerators.cpu:
false` reads as on there again.

| Surface | Change |
|---|---|
| CLI | `accel off cpu` is accepted while another lane (GPU, Neural Engine, CUDA, TensorRT, OpenVINO, llama.cpp or a registered remote lane) is on and can run here. With none it exits non-zero with `the CPU lane stays on while no other lane is on; switch one on first`, where 0.49.0 always refused. Turning off the last other lane is allowed; the CPU is then held on, and its saved off applies again when a lane comes back. `accel on cpu` saves `true`; 0.49.0 cleared the saved value. |
| Defaults | With nothing saved the CPU is off while the Neural Engine is on and available here, on otherwise. A saved choice wins, so a saved `cpu: true` runs the CPU beside the Neural Engine, which 0.49.0 dropped once the Neural Engine was ready. With the Neural Engine ready or on its way the GPU embeds beside it only when `gpu-beside-ane` is on, also while it starts or compiles; a failed Neural Engine is not used. With no lane ready and none on its way the CPU still carries the run whatever its switch says. |
| `GET /api/accel` | The CPU row's `saved` is the saved choice or the default, `source` is `default`, `saved` or `environment`, `locked` is true only while no other lane is on, and `locked_reason` is the message above while locked and `null` otherwise. `cpu_fallback` is unchanged. |
| `settings.json` | No schema change. `accelerators.cpu: false` is now kept and honoured beside another lane. |
| Environment | `SEMLITH_ACCEL` still overrides every switch. |
| Library | `accel::CPU_LOCKED` is new. `accel::CPU_ALWAYS_ON` is kept for programs that hold their own CPU on with it. |
| Portal parity | The CPU row on Settings › Performance is a normal switch while unlocked; the "always on" line shows only while it is locked. |

### 0.49.0

`FORMAT_VERSION` does not move and no store is re-indexed. No MCP schema,
registry or `settings.json` key changes. The first image a store indexes now
also fetches the CLIP text encoder (files only, digest-checked), which 0.48.0
fetched at the first pictured query, so the bytes downloaded are the same.

| Surface | Change |
|---|---|
| CLI | `semlith setup --all-models` also fetches the CLIP vision and text encoders after the embedding and rescoring models, states the total and asks first (`--yes` answers). With `--airgap` or `SEMLITH_AIRGAP` it exits non-zero before any step. Without the flag setup downloads what 0.48.0 did. |
| CLI | A search whose image lane fails (airgap refusal, failed fetch, load or digest check) exits 0 with its text results and prints one `image results skipped: ...` line on stderr, where 0.48.0 exited 1. The next search tries the image lane again. |
| MCP | `semlith_search` and `semlith_brief` add the same note to the result text instead of returning an error. |
| Routes | The search route's answer carries `images_skipped` when the image lane failed. `GET /api/privacy` lists the rescoring model, CLIP vision and CLIP text in `downloads`, each with whether it is on disk. |
| Library | `Semlith::images_skipped` and `Fleet::image_note` return the note; `image_search` failing no longer fails the search. |
| Library | Under `SEMLITH_AIRGAP` the rescoring model is looked up by its repository, so a cached reranker loads; 0.48.0 asked about its display name, which no cache directory carries, and refused it. |
| Portal parity | The note shows above search results; the Privacy page gains the three rows. |

### 0.45.0

`FORMAT_VERSION` does not move, and no route, MCP schema, registry or
`settings.json` key changes. No new request is made.

| Surface | Change |
|---|---|
| CLI | `semlith cloud status` ends each store line with ` · connected here` or ` · not connected here`, and adds a line naming `semlith cloud connect <org>` when any store is not connected here. `--json` is unchanged. |
| Portal | Settings › Cloud lists the stores the host offers that are not connected on this machine, with **Sync new stores** (`POST /api/cloud/connect`, add only), which replaces **Connect its stores**. The Stores page's Remote stores footer points there. |
| Library | **Break:** `cloud::render_status` takes a fourth argument, the org's store names connected here; `cloud::connected_here(entry)` returns them. |

### 0.44.1

`FORMAT_VERSION` does not move, and no library API, CLI flag, MCP schema,
route or `settings.json` key changes.

| Surface | Change |
|---|---|
| Graph | In Rust, Python, Go, JavaScript and TypeScript (Svelte and Vue scripts too), a call with no receiver and no type in its path (`lane(id)`, `crate::accel::lane(..)`) no longer counts a same-named method as a candidate when a free function answers to the name, so it resolves instead of reading as ambiguous. This is decided when an edge is read, so every existing store answers this way at once: `neighbors`, `impact`, `trace`, `path`, `brief`, the portal Graph and `store::edges_in_resolved`. Other languages are unchanged. |
| Graph | In the same languages, a bare call inside a method to the method's own name (`fn lane(&self) { lane(id) }`) is a `calls` edge to the free function instead of being dropped as recursion. Existing stores gain these edges as their files are re-indexed. |
| Graph, symbols | A function declared directly in a class, interface or object body has kind `method` (was `function` for Python, Kotlin, Scala and C++). Go methods are qualified `T::F` by their receiver type (was the package, `a::F`); a C++ out-of-line definition is qualified by its scope (`Foo::bar`, `ns::helper`), and one in a lower-case scope is kind `function`. A file's two same-named definitions each keep their own outgoing edges (the first used to take all of them). A bare call inside a class to a name the class defines carries the class as its hint where a bare call can mean `this.f()`, and in a Python class body. All of this is written at extraction, so a store shows it file by file as files are re-indexed; a store read by 0.44.0 afterwards keeps the new rows. No column or format change. |
| Graph | A Rust call never resolves into another crate's `tests/`, `benches/` or `examples/` file, and a Rust enum variant receiver (`Kind::Catchup.f()`) is typed as its enum. A lower-case module path that names no candidate's file, module or import (`std::env::current_dir()`) stays ambiguous even with one candidate; an inline module's function is named by its path. Decided when an edge is read, except the receiver type, which a file gains when it is re-indexed. |
| Graph | JavaScript member calls carry their object as a hint (at re-index), and JavaScript/TypeScript `this.f()` / `super.f()` are receiver calls; a grouped Rust `use {..}` ranks the modules it names (when read). |
| Symbols | A Rust `fn` outside any `impl` or `trait` (in a `mod` body) has kind `function`, was `method`; a method of an `impl` whose type names nothing (`Box<dyn Error>`) is qualified by its trait. At re-index. |
| Portal | Dialogs scroll instead of squashing a child on a short window (`.modal > *` keeps its height). |

### 0.44.0

`FORMAT_VERSION` does not move. The binary registers no remote lane, so its
CLI, MCP, routes and `settings.json` are unchanged.

| Surface | Change |
|---|---|
| Library | `accel::add_remote(id, remote)` and `accel::remove_remote(id)` register and remove any number of remote lanes at run time; an id is `remote` or `remote-` and lowercase letters, digits and hyphens. `accel::set_remote(Some/None)` stays, for the lane called `remote`. `accel::is_remote(id)`. **Break:** `accel::lane(id)` returns `Option<Arc<Lane>>` instead of `Option<&'static Arc<Lane>>`; `accel::Enabled`'s `remote: bool` is `remotes: Vec<String>` (the remote lanes switched on); `accel::Switches` gains `remotes` and is no longer `Copy`. |
| `settings.json` | `accelerators.remotes`, `{<id>: bool}` for every remote lane but `remote`, which keeps `accelerators.remote`; absent while empty. An older binary ignores it. |
| `SEMLITH_ACCEL` | Takes any remote lane id. |
| `GET /api/accel`, `doctor --gpu` | One row per registered remote lane, labelled `Remote <name>`, each with its `Remote::describe()` fields; a check row carries them whether the lane was checked or not. |
| Graph | A call written inside a Rust macro's arguments (`json!({ "x": f(1) })`) is a `calls` edge. Existing stores gain these edges as their files are re-indexed. |

### 0.43.0

`FORMAT_VERSION` does not move. One index, `symbols_past_path` on
`symbols_past(path, retired_at)`, is created on open (about 6 s once on a store
with a million history rows); an older binary never uses it.

| Surface | Change |
|---|---|
| Cloud ledger sync | `POST /v1/orgs/<org>/ledger` rows gain `hits`, `whole_tokens`, `tier` (`measured` or `modelled`) and `query_id`; no field is renamed or removed, so a cloud that reads only the 0.42.0 fields accepts the body. Rows synced before 0.43.0 are not sent again. |
| CLI | `semlith cloud push` lists each tracked file the walk leaves out as `left out <path> <why>` (and `left_out` in `--json`), and the cloud's refusal of a push that would drop an upload this push kept back or left out names it as such. `semlith report --out` and `semlith cloud report --out` take a folder, made when missing, and write `semlith-<kind>[-<window>][-<table>].<ext>` / `semlith-<org>-<kind>.<ext>` there. `semlith cloud report --format markdown` is `md`. |
| MCP | `semlith_files`, when it shows part of a store, starts with `<shown> of <total> files shown (limit <n>)`. |
| Reports | Printed from a browser, an unbroken run over 12 characters in a table cell breaks anywhere (`<span class="brk">`), so every table fits the page. The native PDF wraps an id or path wider than its column instead of ellipsizing it, and an over-wide table's widest wrappable columns give way. Screen styles are unchanged. |
| Dependencies | zip 8.6.0 -> 9.0.4 (RUSTSEC-2026-0369), which adds `nix` 0.31 on macOS and the BSDs. |
| Library | `report::out_path`, `cloud::report_format`; `store::SyncRow` gains `hits`, `whole_file_tokens`, `tier`, `query_id`; `cloud::PushReport` gains `left_out`. |

### 0.42.0

`FORMAT_VERSION` does not move. The ledger gains three additive columns
(`whole_basis`, `chain`, `seal`) and a `ledger_head` meta row, applied on
open; an older binary ignores them.

| Surface | Change |
|---|---|
| **Break: the whole-file side of a saving** | Rows written from 0.42.0 count a PDF, Office, OpenDocument, EPUB, mail, notebook or HTML file as the text the store holds for it, an image as none, plain text as its bytes over four (`retrievals.whole_basis = 'text'`). Rows already written keep their numbers. |
| **Break: what the figures count** | Every figure is `ledger::figures`: refunds, notes, refusals and accept/revoke rows are not queries; rows from `cli` and `portal` are counted under `human` and out of the agents' figures; a retrieval several stores answered is one; net is not clamped; latency is per retrieval that measured work. `/api/ledger` gains `client_names`, `counted`, `p50_ms`, `p95_ms`, `human`, `sessions_total`, `by_agent`, `by_store`, `rows_total`, and each row `id`, `tool`, `session`, `client_version`, `stale_hits`, `tier`, `basis`, `chain`; `?rows=all` returns every row. The per-store `store::ledger_*` readers take `store::AGENT_RETRIEVAL`. |
| **Break: chain v2** | Rows written from 0.42.0 carry `chain = 2` and a `k2:` hash covering `query_id`, the client version and the basis; `seal` covers usage and `synced_at`; the head is recorded. A binary before 0.42.0 reports the first v2 row as the break; rows it writes after a rollback verify on 0.42.0. |
| MCP | A client on 2026-07-28 is named from `params._meta["io.modelcontextprotocol/clientInfo"]` on every request, or its User-Agent; a stateless client's conversation is derived (client, version, transport, 30 minutes idle) and no `Mcp-Session-Id` is sent to it. `server/discover` answers with no store open. `semlith_read`, `semlith_pattern`, `semlith_impact` and `semlith_trace` write ledger rows. |
| Daemon | Forwarded MCP calls take one of two to four readers; readers share one vector index, query session and tokenizer per store. A run over files a store already holds is answered from the store before it until it commits (readers hold an SQLite read transaction; the WAL grows by the run until then). |
| Library | `ledger::{figures, Figures, Window, is_retrieval, is_human, percent}`, `store::{record_counted, ledger_facts, LedgerFact, held_text, pin_snapshot, unpin_snapshot, is_pinned, record_v1_retrieval, BASIS_TEXT, BASIS_BYTES, AGENT_RETRIEVAL}`, `index::{Vectors, shared}`, `Semlith::share_vectors`, `Fleet::{share_vectors, reconcile}`, `clock::{offset_at, local_when}`, `clientid::user_agent`, `mcp::{client_info, revision_of}`. `mcp::tool_list_tokens()` takes no counter: it is bytes over four. `SkipReason` gains `Encrypted`, `ArchiveTooLarge`, `NotText`. |
| Indexing | A file whose first 8 KiB is over 30 % invalid UTF-8 or control characters is refused as binary. |
| Tool-written state | The Core ML worker's protocol is 4: a fresh `accel/coreml-worker-v4` copy is made and the Neural Engine compiles its models once more. |
| CLI | `semlith ledger` prints one block for every store named, with latency and the human count; rows older than today carry their date. `semlith prices` lists its table. `semlith refused --store` narrows a daemon's listing. `cloud status` says "checked N ago". |

### 0.42.0 — reports and exports

| Surface | Change |
|---|---|
| **Break: report JSON** | `semlith.report/2` (docs/portal.md, Reports › Formats and the JSON schema): typed figures (`value`, `unit`, `currency`, `over`, `display`), KPIs, meta and typed table columns with raw values. `window` is an object (`name`, `label`, `from`, `to`, `applies`). The 0.41 shape (blocks of display strings, `window` a sentence) is gone. |
| **Break: report CSV** | One file per table with snake_case headers and unformatted numbers, saved as a `.zip` with `summary.csv`; `--table <id>` (`table=` on `/api/report`, `table` on `semlith_report`) prints one. `semlith report --format csv` needs `--out` or `--table`. `report::FORMATS` (text) is markdown, json, html. |
| **Break: the default window** | No window named is `month` (30 days) on every surface — the CLI, `/api/report`, `semlith_report` and a schedule without one; it used to be everything outside the portal. `report::Window::Unset` is gone. Every figure of a windowed report counts only rows inside the window. |
| **Break: schedules** | A schedule writes one file named for its options (`semlith-<kind>[-<window>][-<store>…].<ext>`), replaced in place, where it wrote a dated file per run. A record gains `redact` and `excerpts`; an older binary ignores them. |
| Redaction | A redacted query is `hash:` and 16 hex characters of blake3 keyed with `~/.semlith/report.salt` (new, `0600`), not an unkeyed digest. |
| CLI | `semlith report --table`; `semlith schedule add --redact --excerpts`. Numbers in reports print `736,397`. |
| MCP | `semlith_report` takes `window`, `scope`, `redact`, `excerpts` and `table`. |
| Routes | `/api/report` answers CSV without `table` as `application/zip`, takes `table`, and with `preview=1` adds `preview`, a `/report-preview/<id>` path served (credential-free, ten minutes) under a CSP with no script. `/api/schedules` gains `path`; `add` takes `redact` and `excerpts`. |
| Exports | HTML embeds IBM Plex and the logos and follows the portal's tokens; the PDF embeds IBM Plex subsets (and an OS CJK font for glyphs Plex lacks), A4 or Letter by locale, page x of y, bookmarks, metadata, tags. |
| Library | `report` is a module directory: `Report::new`, `Figure`, `Cell`, `Column`, `Kind`, `Table`, `Kpi`, `Fact`, `Meta`, `Chip`, `Chain`, `WindowInfo`, `Block::{Table, Facts, Note, Code}`, `render_table`, `table_ids`, `extension`, `number`, `num`, `DEFAULT_WINDOW`, `CSV`, `SCHEMA`, `REPORT_CSP`; `Options` gains `salt`. `store::report_rows_since`, `files_read_since`, `path_bounds`; `home::report_salt`; `http::Response::with_csp`. |

### 0.42.0 — search, read and brief

Search, read and brief against the 2026-10-14 audit. `FORMAT_VERSION` does not
move: the `markup` meta row is additive, and markup a 0.42.0 code store holds as
written reads as text under 0.41.x.

| Surface | Change |
|---|---|
| **Break: what a code store holds** | `.html` and `.htm` are stored as written and chunked as text unless the store's `kind` or `lean` is `docs` (or the page came from `semlith add`). A store re-reads those files, and only those, on its next full pass. |
| Search ranking | Every list is searched 32 deep whatever `k` is, so `-k 3` returns the first three of `-k 8` with the same scores. An image joins only for a query describing something seen (a picture word, a colour or a shape) and at or above the floor; a weak image is no longer listed. |
| Search output | A hit another store answered with the same span of the same repository (a remote mirror, or two stores over one checkout) is shown once with `agreed` naming the other: a JSON field, `agreed: …` in a locate row, `(agreed: …)` in the CLI. A forwarded exact or symbol answer whose rows the local answer holds prints one line, `<store> (remote · <org>) agreed: the same N rows as above`. |
| `read` | Prints every line of the range, blank ones included. |
| `brief` | Buys the top locator and the text span's locator, then that definition's own edges edge by edge, then the other locators, then text (none once an edge is dropped). `-- dropped:` names locators, edges, symbols' edges and spans' text; a span without text says which span has it; an image is named without a line range. |

### 0.41.0

The rest of the 0.39.0 audit. `FORMAT_VERSION` does not move: a store 0.41.0
wrote opens under 0.40.0, which reads the keyed ledger rows below as not
verifying.

| Surface | Change |
|---|---|
| **Break: what is indexed** | Nothing inside a `.git`, `.ssh`, `.aws`, `.gnupg`, `.kube`, `.config/gh`, `.config/gcloud`, `.azure`, `.docker`, `Library/Keychains`, `.password-store` or `.local/share/keyrings` folder is indexed wherever it sits, whatever an ignore file whitelists. A store that held such files drops them on its next full run. Other whitelisted dotfiles are indexed as before. |
| CLI | `semlith search --local` searches this machine's stores only. |
| MCP | `semlith_search` takes `local: true`, the same. |
| **Break: developer overrides** | `SEMLITH_ADD_ORIGIN`, `SEMLITH_PACK_*` and `SEMLITH_SERVICE_DIR` do nothing in a release build; build with `--features dev-overrides` to try a pack. `SEMLITH_ADD_ALLOW_PRIVATE` is unchanged. |
| What `semlith add` refuses | A URL carrying credentials (`https://user:pass@…`), and NAT64, 6to4, IPv4-compatible and Teredo addresses carrying a private one. |
| The service's output | A daemon the login service starts (`SEMLITH_SERVICE=1`, now set by the plist and the systemd unit) prints its portal URL without the token. |
| Tool-written state | A store outside the store home keeps its `daemon.json` under `~/.semlith/run/<hash>.json`; 0.41.0 reads the old place too, for one release. A store gains `ledger.key` (`0600`) when it records its first retrieval, and a store outside the store home an `.integrity-checked` marker. |
| Ledger | A row's `hash` written by 0.41.0 starts `k1:` and is keyed with `ledger.key`; earlier rows keep their formula and verify. A keyed row with no `ledger.key` beside the database is where `semlith ledger --verify` reports the break. |
| File modes | Every file semlith creates is `0600` and every directory `0700`. |
| HTTP | A request repeating `Host`, `Origin`, `Sec-Fetch-Site`, `Content-Type`, `Content-Length`, `Semlith-Token` or `Authorization` gets 400. A missing store is 404 and an unknown report kind 400, where both were 500; a 403's reason is `Forbidden`. `/api/search` takes `local=1`. |
| MCP numbers | `max_tokens` is clamped to 200 to 200,000 and `semlith_files`'s `limit` to 10,000. |
| `semlith_index` over MCP | A path outside the home and the stores' roots is refused before it is looked at, with `refused: <path> is outside what an agent may index`. |
| `semlith_pattern`, exact search | Stop after 10 s with `truncated`, as at 200 matches. |
| Exports | A CSV cell a spreadsheet would read as a formula starts with `'`; Markdown escapes `<`, `>`, `&`, `|` and backticks. |
| Release | crates.io releases are published through Trusted Publishing. |
| Library | `read_regular`, `NotFound`, `fuzzing::http_head` and `filter::denied_dir`/`is_denied_dir` are public; `filter::Denied::Directory`'s reason no longer starts `under ~/`. |

### 0.40.0

Security fixes from the audit of 0.39.0. `FORMAT_VERSION` does not move: a
store 0.40.0 wrote opens under 0.39.0.

| Surface | Change |
|---|---|
| **Break: the agent key's reach** | A key is read-only by default, and every key that existed is read-only from 0.40.0's first start. `semlith_index`, `semlith_add` and `semlith_forget` answer `isError` with a sentence naming the fix, through `/mcp`, the stdio proxy and a local `semlith mcp`. An agent that indexed or fetched over MCP needs `semlith key --scope write` (or Settings ▸ Agents ▸ Allow writes) once. `tools/list` under a read key leaves the three out, `SEMLITH_MCP_TOOLS=all` included. |
| CLI | `semlith key` with no subcommand prints the scope; `semlith key --scope read\|write` sets it. `semlith refused accept <path> --as-is` on a `crashed` row tries the file once more. |
| `~/.semlith/agent.scope` | New, `0600`: `read` or `write`. Absent reads as `read`. An older binary ignores it. |
| Refusals | A new class, `crashed`, whose `rule` is the reason a parser stopped (`parser crashed: SIGABRT`, `parser timed out after 30 s`, `parser ran past its memory limit (…)`). Not reviewable; accepting it retries. In `semlith refused --json`, `/api/refused` and `/api/refused?decisions=1` (outcome `parser crashed`, `can_retry`). |
| A store's `logs/parse-journal` | New, one JSON object per line. Tool-written state; absent means nothing was refused. |
| Store | `files.read_mtime` (nanoseconds), additive; freshness compares it when present. `chunks_fts` has FTS5 `secure-delete` on. An older binary ignores both. |
| Ledger | A call a read key may not make is a row with `tool` `refused` and the tool's name as `query`; no total counts it. |
| Routes added | `POST /api/agents/scope` (`{"write": bool}`, session token only). `/api/agents` gains `key_scope`. |
| `semlith doctor` | An `agent key scope` line and a `parse worker` line; `--json` gains `agent_key_scope` and `parse_worker`. |
| MCP `semlith_index` through the daemon | Answers after the index budget (45 s, `SEMLITH_MCP_INDEX_BUDGET`) with the run still going, rather than waiting for the run to end. |
| `semlith cloud login` | A `verification_uri_complete` that is not a plain page on the configured host is printed, not opened. |
| **Break: installing and upgrading** | `semlith upgrade`, `install.sh` and `install.ps1` refuse a release without a valid signature by a trusted key, so they no longer install v0.39.0 or earlier (unsigned); pin an older version through `cargo install semlith --version`. Both installers need `ssh-keygen` from OpenSSH 8.1 or later. |
| `semlith --version` | A second line names the release signing key. The first line is unchanged. |
| HTTP | A request head that takes more than 10 s gets 408, one over 16 KiB gets 431, and a ninth idle connection from one address closes the oldest. |
| llama-server lane | The key reaches the server through `LLAMA_API_KEY`, and the server binds a port of its own choosing. |
| Library | `worker` module; `mcp::Session` gains `writes`; `pipeline::take_ready` takes the worker's extraction; `pipeline::Prepared` gains `Crashed`; `store::file_stamps` returns `store::Stamp`; `open_regular` and `git` are public. |

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
