# Compatibility

What you can build against and expect to keep working, and what is free to
change under you. If you are wiring semlith into a script, an agent, or another
crate, this is the page that says which parts are a contract.

## What is covered

These surfaces are the public contract. A change that breaks one of them is a
break, and is treated as one.

| Surface | What is promised |
|---|---|
| CLI commands | The names `index`, `watch`, `search`, `stats`, `files`, `add`, `forget`, `start`, `adopt`, `mcp`, `models`, `languages`, `setup`, `upgrade`, and what each one does. |
| CLI flags | Flag names, their short forms, and their meanings — including the repeatable `--store`/`-s` on the read commands and the single `--store` the write commands take. |
| Environment | `SEMLITH_STORE` (a path-separator-delimited list, split the way `PATH` is), `SEMLITH_HOME`, `SEMLITH_PORT`, `SEMLITH_AIRGAP`, `SEMLITH_EMBED_THREADS`, `SEMLITH_MCP_INDEX_BUDGET`, `SEMLITH_INDEX_MEMORY`. From 0.14.0, `SEMLITH_ADD_ALLOW_PRIVATE` and the `SEMLITH_AGENT_KEY` a client stanza names. From 0.15.0, `SEMLITH_LEDGER` — `0`, `off` or `false` stops the ledger recording anything on this machine. |
| CLI commands added in 0.13.0 | `key show` and `key rotate`, and `start --no-mcp-http`. |
| CLI commands added in 0.14.0 | `trust <dir>` and `trust --list`, and `index --include-secrets`. |
| CLI commands added in 0.16.0 | `read <target>` — `path:start-end`, `path:line` or a symbol name — and `pattern <query> --lang <name>`. `search` gains `--prefer code\|docs\|any`, defaulting to `any`. |
| The portal's session credential | From 0.14.0, a `Semlith-Token` request header. A write additionally needs a JSON content type, and `Sec-Fetch-Site: same-origin` from any client that sends fetch metadata. The cookie is gone; see the break below. |
| The MCP endpoint over HTTP | From 0.13.0, `POST /mcp` on the daemon's port, authenticated by an `Authorization: Bearer` header carrying the agent key from `~/.semlith/agent.key`. The path, the header and the key's location are a contract, because a client's configuration file names all three. The key opens `/mcp` and nothing else. |
| The install scripts | `install.sh` and `install.ps1` stay at the root of the `main` branch, so the two `raw.githubusercontent.com` URLs in the README keep working. They keep honouring `SEMLITH_VERSION`, `SEMLITH_HOME` and `SEMLITH_YES`, and they keep verifying the download against the release's `SHA256SUMS` before writing anything. When `semlith.com` exists it will redirect to these URLs rather than replace them. |
| Release archives | One archive per target, named `semlith-<tag>-<target>`, holding a directory of that name with the binary in it, and a `SHA256SUMS` asset beside them in GNU `sha256sum` format. `semlith upgrade` and both scripts read that layout. From 0.14.0 the Linux archives hold `libonnxruntime.so` beside the binary as well, and every file in the archive is unpacked. |
| `semlith setup --yes` | Runs every step with its default and no prompt, so a script or an agent can install semlith unattended. |
| `semlith upgrade --check` | Exits 0 when the installed version is current and 10 when a newer release exists, and changes nothing either way. |
| Exit codes | Whether a given outcome exits zero or non-zero. A blocked index run exits non-zero; a search that finds nothing exits zero, because finding nothing is an answer. |
| MCP tool names | `semlith_search`, `semlith_stats`, `semlith_files`, `semlith_index`, `semlith_add`, `semlith_forget`, `semlith_symbol`, `semlith_neighbors`, `semlith_path`, from 0.15.0 `semlith_languages`, and from 0.16.0 `semlith_read` and `semlith_pattern`. `semlith_impact` was on this list until 0.13.0 removed it; see the break below. |
| MCP input schemas | The arguments each tool accepts and their types. An existing argument does not change meaning or become required. |
| MCP protocol revisions | The list the server advertises: `2026-07-28`, `2025-11-25`, `2025-06-18`, `2024-11-05`. Dropping one is a break. |
| Where the daemon binds | `127.0.0.1`, and only that. Widening it would be a break in the direction that matters, and is not something a flag will ever do. |
| The default port | `7365`. It does not move on its own: a taken port is an error, not a reassignment. |
| Store layout | A store directory holds `store.db` beside the store's vectors — `index.tv` in format 1, an `index/` directory of shards in format 2 — and the rules for which binary can read which store are below. |
| The formats that are read | The list in the README's *What gets indexed* only grows. An extension semlith reads today is still read tomorrow; what is extracted from it is not covered, and is below. |
| Language names | The set `--lang` accepts only grows. A name that resolves today resolves tomorrow, and to at least the files it resolves to now. Which extensions or filenames make up a name is not frozen — a language gaining one is the set growing, and is not a break. |
| What `semlith add` refuses | From 0.11.0: plain `http`, a redirect that leaves `https`, a chain longer than five redirects, a body over 32 MiB, a content type no reader handles, and any invocation under `--airgap`. Each exits non-zero and writes nothing. These are promises about what semlith will *not* do over the network, so relaxing any of them is a break in the direction that matters. |
| Where `semlith add` writes | The `downloads/` directory inside the store, laid out by host. The file never lands in the user's working tree, and a second fetch of the same name is suffixed rather than overwriting the first. |
| `src/lib.rs` | Documented, not frozen. The `Semlith` type, `Hit`, `IndexReport`, and the modules `chunk`, `embed`, `filter`, `fleet`, `lock`, `mcp`, `store`, `watch` are the supported surface — but the library API changes with the minor version, as it did in 0.2.0. See [the honest version of the promise](#the-honest-version-of-the-promise). |

## What is not covered

Everything below is deliberately outside the contract. Each is excluded for a
reason, and the reason is usually that freezing it would freeze something semlith
should be free to improve.

**The default store location.** Before 0.9.0 a bare `semlith index` wrote to
`./.semlith`; from 0.9.0 it writes to `~/.semlith/stores/<name>`. Nothing about
an existing store changed — the format is identical and a store 0.8.0 wrote
opens unchanged wherever it sits — and the resolution order finds a `.semlith`
beside the corpus before it looks at the home, so no existing setup moves until
somebody runs `semlith adopt`. But *where a new store is created* is different
from what it was, and that is the change to know about in this release.

**`SEMLITH_RELEASES_ORIGIN`.** It points the release lookup, the archive and
`SHA256SUMS` at a different host, and it exists so `tests/install.rs` and
`tests/upgrade.rs` can drive the whole flow against a fixture server on
loopback. It is not a way to self-host semlith releases and nothing is promised
about it.

**Which agents `semlith setup` can register for you.** Claude Code is wired up
by running its own CLI; every other documented client gets its stanza and config
path printed, because those file formats and locations move between versions.
Which clients fall on which side of that line will change.

**The daemon's HTTP routes.** Everything under `/api/` is how the portal talks
to the process that serves it, and both halves ship in the same binary. Paths,
shapes and status codes may change in any release. If you want a stable
programmatic surface, that is `--json` on the CLI and the MCP tools, both of
which are covered above. MCP over HTTP as an endpoint clients connect to
directly is 0.11.0 and will be covered when it arrives.

**`~/.semlith/registry.json` and the daemon discovery file.** Both are
tool-written state, like `store.db` and the lock file. semlith writes them,
there is no supported way to hand-edit them, a field semlith does not recognise
is dropped on the next write, and their shapes are free to change. An older
binary never reads either of them, which is why reinstalling 0.8.0 loses the
registry and nothing else.

**The portal itself.** Its routes, its markup, its assets and its appearance are
not a contract. It is a page, served to a browser on the same machine.

**Ranking scores and result ordering.** The `score` on a hit is a reciprocal
rank fusion score. It orders results within one query and means nothing across
queries or across versions. Any improvement to ranking — a better model, a
different fusion depth, a change to chunking — moves both the numbers and the
order, and that is the point of making the improvement. Do not assert on a
score, and do not assume a result stays at position three.

**The text extracted from a given file.** Which formats semlith reads is
documented and grows additively; how a document is turned into text is not. The
marker lines, the order the pieces come out in, what is dropped as not being the
document's text, where a paragraph ends and a line begins — all of it may change
in any release, because the question each reader answers is "what would a person
see if they opened this?", and a better answer to that is an improvement worth
making. A document read differently is re-chunked and re-embedded when the file
next changes, so the chunk text and the line ranges move with it. Do not assert
on extracted text, and do not build a format on top of the markers.

**Human-readable stdout and its formatting.** The text `semlith search` prints,
the columns `stats` lines up, the wording of a summary line. This is written for
a person reading a terminal, and it gets rewritten when a person reads it badly.
Parse `--json` instead; that is what it exists for.

**stderr diagnostics.** Progress, warnings, the lines the MCP server writes about
which revision it negotiated and which stores it opened. These are for
diagnosing, and their wording changes whenever a better explanation is found.
Nothing should ever be keyed off stderr text.

**The default embedding model.** It changed in 0.2.0 and it may change again when
a better one exists at the same size. This costs an existing store nothing: a
store records the model it was built with and keeps it, so a new default only
applies to a store created after the change. If you need a specific model,
name it with `--model` when the store is created.

**Additive fields.** New keys may appear in `--json` output and new lines may
appear in the text an MCP tool returns. Existing keys keep their names, types and
meanings; new ones show up beside them. Read JSON by key rather than by shape,
and ignore what you do not recognise.

**Internal SQLite schema.** That there is a `store.db` is covered. Its tables,
their columns, the FTS5 configuration and the index layout are not. They are
implementation, they have already changed within 0.x, and reading them directly
is reading past the API. Use the `store` module or the CLI.

### 0.13.0 removes a tool and a command

**This is a break in the covered surface, and is recorded as one.** `semlith
impact`, the `semlith_impact` MCP tool and `GET /api/impact` are gone. The
advertised tool count drops from ten to nine.

- **What breaks.** An agent with `semlith_impact` in a saved prompt, a
  committed `.mcp.json` or a tool allow-list gets an unknown-tool error on
  upgrade. A script calling `semlith impact` exits non-zero with an
  unrecognised-subcommand message. There is no shim and no deprecation period,
  by choice: a tool that answers with an apology is a tool an agent keeps
  calling.
- **Why.** Reverse reachability returns in 0.18.0, rebuilt and free. Leaving
  half of it in place — the command without the page, or the page
  without the command — would have made 0.14.0 a decision half-made across two
  releases.
- **What to do instead.** `semlith neighbors <symbol>` gives the direct callers
  and callees, which is the one-hop answer, and `semlith path <from> <to>`
  answers whether one symbol reaches another. Both are unchanged, as are
  `semlith symbol` and the three MCP tools that carry them.

### 0.14.0 narrows four things it used to do

**Four breaks in the covered surface, all in the same direction.** The 0.13.0
security audit found that a browser tab on any other local port, and a cloned
repository, could each act as the user. Closing those meant refusing things
0.13.0 did — so the restrictions *are* the release, and each of them is recorded
here with the way back where there is one.

#### An unregistered local `.semlith` is not opened without one command

- **What breaks.** `semlith search`, `stats`, `files`, `forget` and `index` in a
  directory holding a `.semlith` that semlith did not create exit non-zero,
  naming the store and both ways forward. So does `semlith mcp` there, so an
  agent pointed at such a directory reports the refusal rather than answering
  from the store.
- **Why.** A `.semlith` directory can arrive inside a repository somebody else
  wrote, and a store is what semlith answers from: a cloned one is a corpus an
  attacker chose, answering the questions an agent asks. Its `daemon.json` also
  chose the port those questions travelled through.
- **The way back.** `semlith trust <dir>` records it once and moves nothing;
  `semlith adopt <dir>` moves it into the store home. `--store` and
  `SEMLITH_STORE` are unchanged and still open anything, because naming a store
  is an instruction rather than a discovery.
- **Who is unaffected.** Every store `semlith index` made. They live under the
  store home and are trusted by being there.

#### `semlith_index` and the portal index inside a boundary

- **What breaks.** The MCP tool `semlith_index`, `POST /api/index`, `/api/root`
  and `/api/adopt` refuse a path outside the target store's registered roots,
  outside the store's own directory when it is a `.semlith` beside its corpus,
  and outside the home directory, and refuse any path under `~/.ssh`, `~/.aws`,
  `~/.gnupg`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, `~/.docker`,
  `~/Library/Keychains`, `~/.password-store` or `~/.local/share/keyrings`, or
  named `.env`, `.env.*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`,
  `id_rsa*`, `id_ed25519*`, `*credentials*`, `*secret*`, `*.tfstate` or
  `*.kdbx`. The hidden-file rule the walker already applied now also applies to
  a path named explicitly. Every refusal is reported per path with the rule that
  refused it.
- **Why.** The agent key lives in a config file on disk, so what it can reach is
  what a copied config file can reach — and that was every file the user could
  read.
- **The way back.** `semlith index` on the command line keeps the deny-list and
  is not confined to any root: the person typing it owns the machine.
  `--include-secrets` turns the deny-list off for that run. There is no opt-out
  for the agent-facing surface, by choice.

#### `semlith add` refuses an address that is not on the public internet

- **What breaks.** A URL whose host resolves — at any hop of a redirect — to
  loopback, an RFC 1918 range, link-local (including `169.254.169.254`),
  carrier-grade NAT, a unique local address or the unspecified address exits
  non-zero naming the address it resolved to.
- **Why.** "Fetch this URL" from a tool running on your machine is how that tool
  becomes a way to read what only your machine can reach: a cloud metadata
  service, a router, something bound to loopback.
- **The way back.** `SEMLITH_ADD_ALLOW_PRIVATE=1`, for a developer indexing
  documentation on an intranet host.

#### The portal's session token is a header, and the cookie is gone

- **What breaks.** A script that drove `/api/*` with a `Cookie:
  semlith_token=…` header gets 401. A browser holding a 0.13.0 cookie is
  answered 401 on its next request and reloads from the printed URL. The
  query-string form (`/api/stores?token=…`) is no longer a credential on any
  route.
- **Why.** Every port on `localhost` is the same site, so `SameSite=Strict`
  never separated this daemon from a page served by anything else on
  `127.0.0.1`. A custom header is attached only by this page.
- **The way back.** Send `Semlith-Token: <token>` instead of the cookie. A
  non-GET request additionally needs a JSON content type, and — if it sends
  fetch metadata at all — `Sec-Fetch-Site: same-origin`. A script that sends
  neither `Sec-Fetch-Site` nor `Origin` is judged on the token alone, so `curl`
  keeps working with one header changed.
- **One thing widened rather than narrowed.** The page itself and its own static
  assets are now served without a credential, because a browser attaches no
  header to a stylesheet, a font or a favicon. They are the same bytes in every
  copy of the binary. Everything that answers about this machine still needs the
  token.

#### Two smaller changes worth knowing

- **The Linux release archive holds two files.** From 0.14.0 the prebuilt Linux
  binaries load ONNX Runtime from `libonnxruntime.so` beside them rather than
  linking it in, which is what lets them start on Debian 12, Ubuntu 22.04 LTS,
  RHEL 9 and Amazon Linux 2023 (issue #57). `install.sh` and `semlith upgrade`
  place both files; a binary copied out of the archive on its own reports the
  missing library by name rather than failing on the first index. `cargo install
  semlith`, macOS and Windows are unchanged.
- **A rotated agent key expires fifteen minutes later**, rather than when the
  daemon exits. On a machine somebody leaves running, "until this process exits"
  was a second live credential rather than a grace period.

### 0.15.0 removes a flag, and changes what an agent gets back

**One break in the covered surface and two changes of default.** All three come
out of the same finding: the graph was answering questions it could not support,
the search was answering "where is this" by sending the thing itself, and the
ledger that was supposed to measure both of those recorded nothing an agent did.

#### `semlith start --ledger` is gone

- **What breaks.** A script, a service file or a stanza that passes `--ledger`
  exits non-zero at parse time with an unrecognised-argument message. There is
  no shim and no deprecation period.
- **Why.** The flag now asks for the default. Keeping it as a no-op would leave
  every existing script reading as though it were switching something on, which
  is the worst of the three options: worse than removing it, and worse than
  leaving it meaning what it meant. A parse error is corrected once and never
  misleads.
- **What to do instead.** Delete the flag. If the intent was *not* to record,
  that is `--no-ledger` for one session, or `SEMLITH_LEDGER=0` for a machine.

#### The ledger records by default

- **What changes.** From 0.15.0 every retrieval is recorded into the store's
  `retrievals` table — over stdio, over the daemon's `/mcp` endpoint, from the
  CLI and from the portal, for graph tools as well as search. Through 0.14.0 only
  the portal's own search box ever wrote a row, and only when `--ledger` was
  passed.
- **Why.** A ledger nobody switched on measured nobody: the savings figure had no
  denominator and the audit trail had no rows. 0.12.0's stated principle — that a
  local tool which starts logging without being told is no different from one
  that phones home — is retired in favour of one that is narrower and checkable:
  the rows never leave the store they were written into, the daemon says on every
  start that it is recording and names the flag that stops it, and erasing every
  row is one `DELETE`.
- **What to do if you do not want it.** `semlith start --no-ledger` for a
  session, `SEMLITH_LEDGER=0` for a machine. Both are read at the point of
  writing rather than cached, so the environment variable takes effect without
  restarting anything.

#### `semlith_search` answers with locations, not excerpts

- **What changes.** Over MCP, `semlith_search` gains
  `format: locate | excerpt` and defaults to `locate`. `/api/search` takes the
  same `format` argument but keeps returning the text unless a caller asks for
  `locate`, because the portal's own Search page is the caller and a person
  reading a panel is not paying by the token. A locate row is the
  store-relative path, the line span, the enclosing symbol and its kind, the
  lists that found it, provenance for a row the graph reached, a freshness flag,
  and one line of the text; rows are grouped by file and cut to a `max_tokens`
  budget (default 1500, floor 200) that states `truncated: N of M` when it cuts.
  A client that parsed the reply for full chunk text gets a shorter reply than it
  did in 0.14.0.
- **The CLI is unchanged.** `semlith search` still prints excerpts, and `--json`
  still carries the chunk text. This is a change to the agent-facing default
  only, because a person reading a terminal is not paying by the token.
- **The opt-back.** `format: "excerpt"` returns exactly what 0.14.0 returned.
- **Why.** An agent that already knows the identifier wants the address, not the
  building. The study behind this release measured a warm reply at 4.9–7.2 KB at
  `k=8` against 100–300 bytes for the grep the agent could have run instead.

#### What is additive, and therefore is not a break

Everything else in this release adds fields beside the ones already there, under
the *Additive fields* rule above. Existing keys keep their names, types and
meanings.

- **`edges.hint`**, a nullable `TEXT` column recording what the source said about
  where a call goes. NULL on every row an older binary wrote.
- **Four nullable columns on `retrievals`**: `session`, `tool`, `stale_hits` and
  `tokenizer`.
- **On a hit**: `fresh`, `symbol`, `symbol_kind` and `provenance`. `fresh` is
  present on every hit; the other three are omitted when there is nothing to say.
- **On an edge**: `definitions`, `from_path` and `from_line`.
- **Two more confidence values.** An edge's confidence was `extracted` or
  `inferred`; it is now one of `extracted`, `resolved`, `inferred` or
  `ambiguous`. The two new ones are computed when a query runs and never stored,
  so nothing in a store changes shape — but a consumer that matched on exactly
  two strings will see two it does not recognise.

## The honest version of the promise

semlith is 0.x. Under SemVer, a 0.x minor bump is permitted to break anything,
and this page is not going to pretend otherwise — a stability claim the version
number does not back is worth less than no claim.

What is true is the project's practice. The surfaces listed under *What is
covered* are treated as stable across 0.x releases: the intent is that a script,
an agent configuration, or a crate written against 0.5.0 keeps working on 0.6.0
and after. That is a commitment about how the project behaves, not a guarantee
the version number carries.

The one deliberate exception, already exercised: the `src/lib.rs` API took
breaking changes in 0.2.0, and it is the surface most likely to move again. The
crate is published and therefore importable, so it is documented here — but a
library consumer should pin an exact version and read the CHANGELOG before
upgrading. The CLI, the MCP tools and the store format are the surfaces the
practice most strongly covers.

This page says nothing about when 1.0 happens, because nothing has been decided.

## Store format

The store's `meta` table carries a `format_version` key, written by 0.6.0 and
later. A store without the key is format 1 — every store written before 0.6.0,
read as-is, with no migration and nothing rewritten.

| Format | Vectors live in | Written by |
|---|---|---|
| 1 | one `index.tv` | 0.1.0 through 0.6.0 |
| 2 | an `index/` directory of fixed-size shards | 0.7.0 and later |

A binary that opens a store whose `format_version` is higher than the format it
knows refuses it, naming both numbers, rather than reading it as best it can.
Misreading a newer store is the failure worth preventing: it does not look like
an error, it looks like a corpus that has stopped containing things.

**Forwards, every 0.x store still opens.** 0.7.0 reads, searches and indexes
into a store written by any earlier version, on its existing single `index.tv`,
without re-embedding anything and without touching its `format_version`. A
format-1 store is never migrated: the vectors in an `index.tv` are quantized and
cannot be split back out, so any migration would re-embed the corpus, which is
something to decide rather than to have done to you. Deleting the store
directory and indexing again is the way onto format 2, and there is no hurry.

**Backwards, format 2 is where it stops.** A store created by 0.7.0 cannot be
read by 0.6.0, which refuses it naming both numbers. That is the break this
format change makes, and it is the reason `format_version` was added a release
early. One sharper edge: 0.5.0 and earlier predate the key entirely and have
nothing to check, so such a binary reads a format-2 store as an empty corpus
rather than refusing it. If you keep a pre-0.6.0 binary around, do not point it
at a store 0.7.0 created.

Between 0.5.0 and 0.6.0 the compatibility is still total in both directions:
`format_version` was an additive meta key, which is exactly why it was safe to
add before the format needed it.

### 0.12.0 adds tables and does not move the number

0.12.0 puts the code graph (`symbols`, `edges`) and the retrieval ledger
(`retrievals`) inside `store.db`, and `format_version` stays **2**. That is
deliberate, and the reasoning is worth writing down, because "new tables, new
format number" looks like the careful choice and is the wrong one here.

The schema is applied with `CREATE TABLE IF NOT EXISTS` on every open, and
`format_version` is written only when a store is *created*. So bumping the
number would not upgrade anything: every store in existence would keep saying 2
for ever, with no path to 3 short of deleting it and re-embedding the corpus —
and every 0.11.0 binary would start refusing stores that 0.12.0 had merely
opened. A number no store can reach, bought at the price of breaking the
previous release, is worse than no number.

What the number is *for* is the vector layout, where misreading a store is
silent: format 1's single `index.tv` and format 2's shard directory cannot be
told apart by reading them. Tables are not like that. An older binary does not
read `symbols`, `edges` or `retrievals` at all, so their presence cannot mislead
it, and their absence in a store an older binary wrote is exactly the empty
state a newer binary already handles.

**So, in both directions, with no migration:** a store written by 0.11.0 opens
under 0.12.0 with an empty graph and an empty ledger, searches correctly, and
gains its graph on the next `index` pass — which re-reads the file bytes,
because symbols cannot be recovered from chunk text alone. A store written by
0.12.0 opens under 0.11.0 and searches exactly as it did before; the extra
tables sit there unread. Both directions were run against the released 0.11.0
binary rather than asserted here.

### 0.13.0 adds an images table and a second vector directory

Same reasoning, same answer: `format_version` stays **2**.

0.13.0 records one row per indexed image in an `images` table inside
`store.db`, and writes those vectors into an `images/` directory inside the
store, beside the text index. Both are additive, the table is applied with the
same `CREATE TABLE IF NOT EXISTS` batch every open already runs, and an older
binary reads neither — so a store written by 0.13.0 opens under 0.12.0 and
searches its text exactly as before, with the image rows sitting there unread.

There is no backfill, and there cannot be a useful one: an image vector needs
the file's bytes, which the database does not hold. A store written before
0.13.0 opens with no images and honestly reports none until its next `index`
pass, which re-reads the files.

The internal table layout is [not a covered surface](#what-is-not-covered), and
this does not change that. It is described because people plan around it, not
because it is promised.

### 0.15.0 adds columns and does not move the number

Same reasoning one step further along: `format_version` stays **2**.

0.12.0 and 0.13.0 added whole tables an older binary does not read. 0.15.0 adds
*columns* to tables that already exist — `hint` on `edges`, and `session`, `tool`,
`stale_hits` and `tokenizer` on `retrievals` — which is the case worth spelling
out, because a column is something an older binary might plausibly meet.

Every one of them is nullable and added by an `ALTER TABLE` that runs on open
beside the `CREATE TABLE IF NOT EXISTS` batch, so no store is migrated and
nothing is rewritten. An older binary reads these tables with `SELECT` statements
that name their columns, so a column it has never heard of is a column it never
asks for.

**So, in both directions, with no migration:** a 0.14.0 binary opens a store
0.15.0 wrote, searches it, walks its graph and prints its ledger exactly as it
did before, ignoring the five new columns. A 0.15.0 binary opens a store 0.14.0
wrote, finds a NULL hint on every edge, and resolves those edges exactly as
0.14.0 did — the hint is a tie-breaker, and its absence costs the ranking a tier
rather than an answer. The rows fill in for a file on the next `index` pass that
touches it, which re-reads the bytes.

The ledger's hash chain is the one place where a new column could have broken an
old store, because the chain covers a row's fields. It is versioned by the row
instead: `tool` is NULL on every row written before 0.15.0 and set on every row
written since, and that is what decides which formula verifies it. A store
holding rows of both kinds verifies end to end. A verify that reported every
0.14.0 ledger as broken would be worse than no verify at all.

## Breaks in 0.16.0

Three, none of which needs a re-index and none of which changes the store format.

**No portal page is added for `read` or `pattern`.** Not a break — they are new
in this release and never had one — but worth stating, because portal parity
otherwise implies it. `read`'s view is the Search page's second stage and
`pattern` is an agent-facing query surface; both are on the CLI and over MCP,
and the Agents page lists them. `/api/read` and `/api/pattern` exist and are
covered by the same stability promise as the rest of `/api`.

**The portal's Languages page is gone.** Its content — the language list, with
which of them carry graph edges — is on the About page, where the design puts it.
`semlith languages`, `semlith_languages` and `/api/languages` are unchanged, so
nothing scripted against them breaks; only the page and its navigation entry are
removed. A bookmark to the Languages route lands on About.

**MCP array arguments no longer declare `items`.** `path`, `ext`, `lang` and
`store` are still arrays of strings and still behave identically. Their schemas
now say `{"type": "array"}` rather than `{"type": "array", "items": {"type":
"string"}}`. This is what paid for the two new tools inside the same `tools/list`
budget, and it is listed here because a client that validates strictly against
the advertised schema sees a change — although a schema that constrains less
never rejects what the old one accepted.

**`semlith_symbol` returns more than it did.** It answered with a flat list of
definitions; it now answers with a block carrying the definition, the resolved
callers and callees, and the ring two hops out. A caller that parsed the old
output line by line has to read the block instead. `/api/symbol` keeps its
`symbols` key unchanged and adds `callers`, `callees` and `ego` beside it, so the
HTTP shape is additive rather than replaced.

## What a break would look like

If one of the covered surfaces has to change, this is what happens:

1. **It goes in the CHANGELOG**, in that version's section, stated as a break
   rather than folded into a list of improvements.
2. **The entry says why.** A rename with no reason is a rename that should not
   have happened; if the reason does not survive being written down, the change
   does not either.
3. **The entry says what to do.** The new spelling, the flag that replaces the
   old one, or the one-line edit to a configuration stanza. For a store format
   change it says whether a re-index is needed and what it costs.

Before upgrading, read the CHANGELOG section for the version you are moving to.
It is short, and a break is called out rather than buried.
