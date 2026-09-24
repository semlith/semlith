# Compatibility

What you can build against and expect to keep working, and what is free to
change under you. If you are wiring semlith into a script, an agent, or another
crate, this is the page that says which parts are a contract.

## What is covered

These surfaces are the public contract. A change that breaks one of them is a
break, and is treated as one.

| Surface | What is promised |
|---|---|
| CLI commands | The names `index`, `watch`, `search`, `stats`, `files`, `add`, `forget`, `drop`, `start`, `adopt`, `mcp`, `models`, `languages`, `ledger`, `symbol`, `neighbors`, `path`, `setup`, `upgrade`, and what each one does. |
| CLI flags | Flag names, their short forms, and their meanings — including the repeatable `--store`/`-s` on the read commands and the single `--store` the write commands take. |
| Environment | `SEMLITH_STORE` (a path-separator-delimited list, split the way `PATH` is), `SEMLITH_HOME`, `SEMLITH_PORT`, `SEMLITH_AIRGAP`, `SEMLITH_EMBED_THREADS`, `SEMLITH_MCP_INDEX_BUDGET`, `SEMLITH_INDEX_MEMORY`. From 0.14.0, `SEMLITH_ADD_ALLOW_PRIVATE` and the `SEMLITH_AGENT_KEY` a client stanza names. From 0.15.0, `SEMLITH_LEDGER` — `0`, `off` or `false` stops the ledger recording anything on this machine. From 0.20.0, `SEMLITH_INDEX_PARALLEL` — how many index runs the daemon may have going at once. |
| CLI commands added in 0.13.0 | `key show` and `key rotate`, and `start --no-mcp-http`. |
| CLI commands added in 0.14.0 | `trust <dir>` and `trust --list`, and `index --include-secrets`. |
| CLI commands added in 0.18.0 | `doctor`, with `--json` and `--fix`, and `setup --register-all`. |
| CLI commands added in 0.16.0 | `read <target>` — `path:start-end`, `path:line` or a symbol name — and `pattern <query> --lang <name>`. `search` gains `--prefer code\|docs\|any`, defaulting to `any`. |
| CLI commands added in 0.19.0 | `scan [STORE]`, with `--forget` and `--json`. It exits non-zero while the store still holds anything today's rules would refuse, which is a contract a script can gate on. `pattern` gains `--path` (a repeatable glob) and `--offset`. |
| CLI commands added in 0.24.0 | `hook`, which reads one `PreToolUse` event as JSON on stdin and writes the client's answer on stdout. It never blocks unless `--strict` was asked for, never exits non-zero, and is silent for a file no registered store holds. `setup` gains `--no-hooks`, which removes the hook it writes by default, and `--strict`. |
| CLI flag values added in 0.24.0 | A leading `!` on a `--path`, `--ext` or `--lang` value excludes instead of including, applied after the inclusions of its own kind. The same holds for the `path`, `ext` and `lang` fields of every MCP tool that takes them. A value with no `!` means exactly what it meant before. |
| Environment added in 0.24.0 | `SEMLITH_DEFAULT_IGNORES` — `0`, `off` or `false` turns off the built-in table of generated and vendored directories, so `node_modules` and its kind are indexed as they were before 0.24.0. |
| What the MCP handshake carries from 0.24.0 | `initialize` returns an `instructions` string, the same one `server/discover` has returned since 0.20.0. A client that ignores it is unaffected. |
| The ledger's row kinds from 0.24.0 | `raw-read`, written by `semlith hook` for a whole-file read of a file a store holds. It is a row in the existing table with a `tool` of its own, so an older binary reading a 0.24.0 ledger verifies the chain unchanged and simply does not know what the row means. |
| CLI flags added in 0.20.0 | `index --each`, one store per path; `index --projects <FOLDER>`, which takes the paths from the folder's children and implies `--each`. Neither changes what `semlith index` does without them. |
| MCP tool arguments added in 0.19.0 | `semlith_pattern` takes `path`, an array of globs, and `offset`, an integer. Both are optional and both default to what 0.18.0 did, so a client that passes neither sees no change. |
| `/api/report` arguments added in 0.27.0 | `window` — one of `all`, `day`, `week`, `month`, `quarter` — and `scope`, a repeatable store label. Both are optional. A request that names neither gets exactly what 0.26.x returned: every open store, and each report's own span (seven days for the change brief, everything for the rest). An unknown `window` is 400 and names the five; a `scope` no open store answers to is 400 and names the stores that are open. `semlith report` takes `--window` and `--scope` with the same meanings. |
| Report formats from 0.27.0 | `markdown`, `csv`, `json`, `html` and `pdf`. The first four are unchanged, and `GET /api/report?format=<one of them>` still answers with the JSON envelope carrying the rendering as `text`. `format=pdf` answers with the PDF itself, `Content-Type: application/pdf`, because bytes cannot ride inside a JSON string; `semlith report --format pdf` needs `--out` for the same reason. The PDF is typeset from the same block structure the other four render, not a print of the HTML. |
| CLI commands added in 0.28.0 | `accel [status \| on <lane> \| off <lane> \| remove <lane>]`, with `--json`, over the lanes `cpu`, `gpu` and `cuda`. `status` is the default. Refusals exit non-zero: the CPU turned off with no usable GPU lane, CUDA on a platform without it, or any switch while `SEMLITH_ACCEL` is set. `doctor --gpu`, with `--json`, which conflicts with `--fix` and `--brief`. It exits non-zero when any lane fails the known-answer check, and not for a lane that is unavailable. `__embed-worker` is hidden and is not part of the interface. |
| Environment added in 0.28.0 | `SEMLITH_ACCEL` is a comma list of lanes, such as `cpu,gpu`. It sets which lanes are on and takes precedence over the saved switches, the same way the other limit variables do. A lane the list omits is off. `SEMLITH_SESSION_IDLE_SECS` sets how long a writer keeps an unused session, 60 by default. |
| `~/.semlith/schedules.json` from 0.27.0 | Tool-written state, like `registry.json` and the lock file — not a configuration file, and not a promise. It is absent until a schedule is added, and its absence is the normal state rather than an error. A 0.26.x binary reads no such file and ignores one left behind. |
| The portal's session credential | From 0.14.0, a `Semlith-Token` request header. A write additionally needs a JSON content type, and `Sec-Fetch-Site: same-origin` from any client that sends fetch metadata. The cookie is gone; see the break below. |
| The MCP endpoint over HTTP | From 0.13.0, `POST /mcp` on the daemon's port, authenticated by an `Authorization: Bearer` header carrying the agent key from `~/.semlith/agent.key`. The path, the header and the key's location are a contract, because a client's configuration file names all three. The key opens `/mcp` and nothing else. |
| The install scripts | `install.sh` and `install.ps1` stay at the root of the `main` branch, so the two `raw.githubusercontent.com` URLs in the README keep working. They keep honouring `SEMLITH_VERSION`, `SEMLITH_HOME`, `SEMLITH_YES` and `SEMLITH_NO_SERVICE`, and they keep verifying the download against the release's `SHA256SUMS` before writing anything. When `semlith.com` exists it will redirect to these URLs rather than replace them. |
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
registry and nothing else. From 0.20.0 `~/.semlith/settings.json` and
`~/.semlith/queued.json` join them on the same terms — the first is what the
portal saved of the three indexing settings, the second is what a stopping daemon
left for the next one to report — and neither is a configuration file semlith
promises to keep reading. A file that cannot be read or parsed is the same answer
as no file.

**The portal itself.** Its routes, its markup, its assets and its appearance are
not a contract. It is a page, served to a browser on the same machine.

**`GET /api/report` honours `store` instead of discarding it.** The route read a
`store` parameter and threw it away, so a request that asked for one store of six
got all six and nothing said so. From 0.27.0 the store filter is `scope`, and
`store` is read as its alias. A caller that never passed either is unaffected; a
caller that was passing `store` now gets the report it was asking for. The
report's own `stores` field, and the line every format prints under the title,
name the scope that was applied.

**Every report now states its window under the title.** The line that read
"Generated <when> on this machine, over <stores>. Nothing left it." now reads
"Generated <when> on this machine, over <stores>, covering <window>. Nothing left
it.", and the JSON payload gains a `window` string beside `stores`. A report
whose figures carry no date — savings, health and gaps — says so in that line
rather than printing a span it did not apply. A script that parsed that line
verbatim has to be updated; one that reads the JSON gains a field and loses none.

**Portal parity, and its one exception.** Every CLI command and every MCP tool
has a surface in the portal that a person can open; `tests/portal.rs` is the
gate and the place each deliberate absence is argued. From 0.27.0 there is one
more absence than there was, and it is worth stating here rather than only in a
test file: **`semlith models` and `/api/models` have no portal view.** The About
page carried a forty-eight-row table of every embedding model a store could be
built with, of which one row is a model any given machine has fetched; the v4
design has no place for it and the page it sat on is now seven facts and the
language table. Nothing was withdrawn — `semlith models` prints the full list
and `/api/models` answers exactly as before. `/api/pattern` has had no portal
view in every release so far, for the reason recorded in `tests/portal.rs`: a
tree-sitter query in S-expression syntax is not something anyone types into a
browser box.

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
- **Why.** Reverse reachability returns in 0.14.0 as a paid surface. Leaving
  half of it in the free product — the command without the page, or the page
  without the command — would have made 0.14.0 a decision half-made across two
  releases.
- **What to do instead.** `semlith neighbors <symbol>` gives the direct callers
  and callees, which is the one-hop answer, and `semlith path <from> <to>`
  answers whether one symbol reaches another. Both are unchanged, as are
  `semlith symbol` and the three MCP tools that carry them.

### 0.26.0 adds three commands and three tools, and none of them costs anything

**Additive, and the only change to the covered surface in this release.**
`semlith impact`, `semlith trace` and `semlith report` join the command line;
`semlith_impact`, `semlith_trace` and `semlith_report` join the tool list, which
goes from thirteen to sixteen; `GET /api/impact`, `/api/trace`, `/api/map`,
`/api/report` and `/api/ledger/replay` join the daemon's routes.

- **What this restores.** `semlith impact` and `semlith_impact` were removed in
  0.13.0 to be sold, and never were. The monetization hold of 2026-09-15 made
  the binary free whole, so they come back with a page of their own and no key
  of any kind. An agent that carried `semlith_impact` in a saved prompt from
  0.12.0 works again; one written against the 0.12.0 answer shape does not, and
  the shape here is the current one.
- **What it costs.** The tool list a client pays for once per session grows with
  it, and the gate on that figure moved from 1 120 tokens to 1 600. The Agents
  page states what this binary's own list measures rather than a number written
  down when it was last checked.
- **What did not change.** Nothing in the retrieval path. The 0.25.0 figures are
  re-measured on this release's binary and reproduce; a moved number would have
  been a defect in this release rather than a new measurement.

### 0.26.0 holds the portal's index route to the boundary it documents

**This is a break in behaviour, and is recorded as one.** `POST /api/index` now
refuses a path outside the target store's boundary when the request names a
store, and a request that names no store indexes the path into the store
`home::resolve` picks for it rather than into whichever store happened to be
writable.

- **What breaks.** A script that posted a path and a `store` name that did not
  contain it now gets 403 with the rule that refused it. A script that posted
  one path with no `store` now gets a run in that path's own store, which may
  be a new one, rather than a run in an unrelated store.
- **Why.** `docs/security.md` and this document have said since 0.20.0 that the
  route refuses a path outside the store's registered roots. It did not: the
  route recorded whatever it was handed as a root *before* the run applied the
  boundary, so the check could never refuse anything (issue #120). One of the
  two was wrong, and it was the code.
- **What to do instead.** Post no `store` and let the path choose its own, which
  is what `semlith index <path>` does, or add the folder to the store's roots
  first with `/api/root`.

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

| Format | What it says about the store | Written by |
|---|---|---|
| 1 | vectors in one `index.tv` | 0.1.0 through 0.6.0 |
| 2 | vectors in an `index/` directory of fixed-size shards | 0.7.0 through 0.21.0 |
| 3 | Markdown chunks cut at their headings, embedded with the heading path in front | 0.22.0 through 0.24.0 |
| 4 | code chunks embedded with the definition they sit inside | 0.25.0 and later |

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

### 0.25.0 moves the number, and re-embeds once

Format 4 says what the embedding model was *shown*, which is the other thing a
format number is for: a store whose code chunks were embedded without their
enclosing definition and one whose chunks were embedded with it hold vectors
that mean slightly different things, and nothing about the rows says which.
Half a store each way ranks its own files against each other unevenly, and that
is silent — the same failure mode the shard layout has, arriving through the
vectors rather than through the file layout.

So the first full index pass under 0.25.0 re-chunks and re-embeds everything it
walks, whether or not the files changed, and moves the format row at the end of
a pass that swept the whole store. A pass over one directory leaves the rest on
the old rule and the row stays where it was, because moving it would be a claim
about files the run never looked at. The run says on its first line that it is
re-indexing and why. Nothing a store indexed is touched.

**Forwards:** a store written by 0.22.0 through 0.24.0 opens under 0.25.0,
searches, and answers exactly as it did until that pass; it is not migrated on
open. **Backwards:** 0.24.0 and earlier refuse a format-4 store by name, which
is the break this number exists to make. Rolling back therefore means
re-indexing under the older binary, and the release notes say so.

The rescoring model is not part of any of this. It is read at query time from
the model cache and never written into a store, so a store searched with it and
a store searched without it are the same store, and `semlith stats` says which
ranking answered.

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

## 0.17.0

**The code graph covers every language `--lang` accepts.** Through 0.16.0 six of
the forty-six carried symbols and edges; from 0.17.0 all forty-six do. Nothing
about the surface changes — no new command, no new tool, no new argument, no
schema change and no `FORMAT_VERSION` move. What changes is that `semlith
symbol`, `semlith neighbors`, `semlith path` and a graph-expanded `semlith
search` now answer for a Ruby, PHP, Kotlin, Swift, Scala, Haskell, Lua, Elixir,
Zig or Dart corpus, and for a corpus of YAML, TOML, JSON, Markdown, Terraform,
Dockerfiles, Makefiles, GraphQL, protobuf, SQL, CSS and HTML.

This is additive in the sense the table above describes: the set of languages
`--lang` accepts has not changed, and the set that carries a graph only grows.

**A store keeps working and gains the new languages on its next index pass.** A
store written by 0.16.0 opens under 0.17.0 unchanged, and a 0.16.0 binary opens
a 0.17.0 store — the format version does not move and no column is added.
Symbols and edges for the newly covered languages appear when those files are
next indexed, exactly as every other graph fact does. A user who wants them
immediately runs `semlith index` over the root; there is no build step and there
must not be one.

**`semlith languages` prints a column it did not print.** The output gains a
`graph` marker between the language name and its extensions, which the portal's
About page and `semlith_languages` already showed. A script parsing that output
by column position sees a new column; parsing by the first field is unaffected.

**`THIRD-PARTY-NOTICES` ships with the binary.** Forty-six parsers are compiled
in, each under its own permissive licence, and the notice those licences ask for
travels in the crate and in every release archive rather than only in the
repository.

**The binary is larger.** Forty grammars are forty C parsers. The measured delta
is in the CHANGELOG entry for this version.

## 0.17.1

**Five answers change, and every one of them is a case where the old answer said
an operation had succeeded when it had not.** Nothing is added and nothing is
removed; what moves is what a script sees.

**`semlith index` on a path that cannot be read exits non-zero.** Opening a store
is what creates it, so through 0.17.0 a typo left an empty store behind,
registered under the typo's own name, and exited 0. The roots are checked before
a store is chosen: a run where every root is unreadable creates nothing and
registers nothing, and a run mixing readable and unreadable roots indexes the
readable ones, names each unreadable one on stderr, records only the readable
roots — and still exits 1. A script that passed a path that had moved and read
the zero exit as success now sees the failure. `POST /api/index` answers 400 and
the `semlith_index` MCP tool refuses on the same condition.

**`semlith forget` of a path that is not indexed exits 1.** It exited 0. `forget`
resolved its store from the working directory rather than from the path it was
handed, so a forget run from anywhere but the corpus asked a store that had never
held the file, removed nothing, and reported success. It is now anchored on the
path, the way `index` already is, and a path no store holds prints `nothing to
forget: <path> is not indexed` on stderr.

**`POST /api/forget` answers 404 where it answered 200 with a zero count.** For a
single path, and for a set where nothing was removed.

**`GET /api/search` applies the `offset` it validates.** It checked the parameter
and then ignored it, so every page repeated the first one. A client that was
passing `offset` and reading that repetition as the answer now gets different
rows, and the response echoes `offset` back the way `/api/files` already does.
The portal's own Search page does not page yet; the route honours the parameter
for the clients that already believed it did.

**Printing into a closed pipe exits 0 and prints nothing.** `semlith files |
head` panicked and exited non-zero, on every platform. It now ends the way `cat`
and `grep` do, so a pipeline that was tolerating a panic on stderr and a non-zero
status sees neither.

**On Windows the store home is `%USERPROFILE%\.semlith`.** Not a break, but the
one change a Windows user can see in their own filesystem. Windows sets no
`HOME`, and semlith read it in nine places, so the store home came out as
whatever the process happened to have as a working directory — and the store, the
registry, the agent key and 52 MB of model weights were written there.
`user_home()` reads `HOME`, then `USERPROFILE`, then `HOMEDRIVE` plus
`HOMEPATH`, and errors naming `SEMLITH_HOME` when it knows none of them, so the
store home is the directory `install.ps1` already puts the binary in.
`SEMLITH_HOME` is still read first, so a user who set it to work around this
keeps every store exactly where they put it and nothing moves. A user who did not
has a `.semlith` in whatever directory they first ran semlith from: this release
stops creating it, and does not find or move the one already there. Copy it,
delete it, or index the corpus again.

**The directory deny-list fails closed.** A path that cannot be checked against
`~/.ssh`, `~/.kube` and the rest is refused rather than waved through. On
Windows, where the home never resolved, those rules had never run at all.

**Paths print in the form the platform opens.** The verbatim `\\?\` prefix — and
the `\\?\UNC\` form, back to `\\` — is stripped where a path becomes text: search
hits, file rows, image rows, symbol rows and call-site paths, on the CLI and its
`--json`, in MCP tool results, and in the portal's `/api/search` and `/api/files`.
The store still holds the verbatim form, because that is what makes a path longer
than 260 characters work, and `semlith read` accepts either. A Windows script
that matched a leading `\\?\` stops matching; one that hands the printed path to
an editor starts working.

**No store format change.** `FORMAT_VERSION` does not move, no table and no
column is added, and a store written by 0.17.1 and a store written by 0.17.0 are
byte-compatible in both directions.

## 0.17.2

**Nothing a user drives changes.** No command, no flag, no MCP tool, no schema,
no store format. `FORMAT_VERSION` does not move, and a store written by 0.17.1
and one written by 0.17.2 are byte-compatible in both directions.

**The client configuration stanzas move from `README.md` to `docs/clients.md`.**
This is a contract for exactly one kind of reader: a packager or a fork that
patched the README's client section moves that patch to the new file. The
stanzas themselves are unchanged — the same bytes, at the same heading levels,
because `src/clients.rs` parses those headings — and `GET /api/agents` returns
the same twenty-seven clients in the same three groups. `src/clients.rs`'s
`include_str!` names the new path, so the file is part of the published crate;
a build that excluded it would not compile.

**The portal's Graph page draws `contains` and `aliases` edges.** It fetched
both from `/api/graph` and dropped them before painting, so the canvas showed
four of the six kinds the store holds while the hover card counted all six. Both
kinds now have a filter chip, and the caller and callee counts are taken over
what is drawn. Nothing about the stored graph changes; this is the renderer
catching up with it.

**A rotated agent key is documented as lasting fifteen minutes.** That is what
`http::KEY_GRACE` has been since 0.14.0. Four places in the repository still said
"until this daemon exits", including the message the portal prints after a
rotation. The behaviour is unchanged and the sentences now match it.

## 0.18.0

**`semlith setup` registers agent clients, and `--yes` registers them too.**
Until 0.18.0 it registered exactly one client, Claude Code, and `--yes` skipped
the agents step entirely. It now runs the registration command of every client
that documents one, at the scope that client spells "every project", and `--yes`
does the same. `semlith setup --yes` remains what a script runs unattended, and
it still writes no file semlith does not own — that is `--register-all`, which
lists every path and asks before writing any of them.

**Every registration semlith writes is the stdio form.** A registered client
launches `semlith mcp` rather than holding `http://127.0.0.1:7365/mcp` and an
`Authorization` header. Three things follow, and they are the release. No
configuration file semlith writes carries the agent key or names
`${SEMLITH_AGENT_KEY}`. A key rotation reconfigures nothing, because the key is
read from `~/.semlith/agent.key` by the `semlith mcp` process rather than
expanded from the environment by the client — `semlith key rotate` no longer
re-registers anything, and says so. And the shell startup block that exported
`SEMLITH_AGENT_KEY` is gone: `semlith setup` writes `PATH` and nothing else, and
replaces a block an earlier version wrote rather than leaving any block it finds
in place — which is what it used to do, so an upgrade kept the export for ever.
Open a new shell after running it.

`SEMLITH_AGENT_KEY` stays on the covered list and the HTTP endpoint is not
withdrawn: a daemon on another machine still needs a header, `docs/clients.md`
still documents the HTTP stanzas, and a user who pastes one exports the variable
themselves. What changed is that nothing semlith writes depends on it.

**An existing semlith registration is replaced, not added beside.** For the ten
clients that document a remove verb, `semlith setup` runs it before the add, at
every scope that client has. Claude Code's `local` scope is included
deliberately: an entry under `projects."…".mcpServers` in `~/.claude.json` is
what made semlith invisible from every directory but one, and it is removed.

**Two clients are no longer registered by their own CLI.** `opencode mcp add`
on 1.18.11 and `kilo mcp add` have no flag that means every project, so running
them would register the directory the user was standing in. They are registered
by writing their user-level configuration file under `--register-all` instead.
Three others — Crush, Zed and Roo Code — document no user-level path at all and
semlith registers them nowhere; `semlith doctor` names all three with the reason.

**`semlith doctor` is added**, with `--json` and `--fix`. It reports, per client,
whether it is installed, whether semlith is registered, at what scope, and what
to run otherwise, plus the four Privacy rules that are readings of this machine.
It exits non-zero when something is not as it should be, which is a contract a
script can gate on; a client that is simply not installed is not a fault.

**The Privacy page gains a manual step on every failing rule and a button where
a repair qualifies.** `POST /api/privacy/fix` and `POST /api/agents/register` are
new routes, and `GET /api/doctor` is the Doctor page's. A repair narrows access,
is idempotent, touches only a path semlith owns, and is confirmed by re-running
the rule's own check. `private addresses` has no button and cannot: the variable
is in the environment the daemon inherited.

**`GET /api/setup`'s `claude_registered` is replaced by `registered_clients`.**
The old field was a tri-state about one client, answered by spawning
`claude mcp list`. The new one is a list of the clients whose own configuration
file names semlith, read from disk, because asking sixteen client CLIs on a route
the portal calls on every load is sixteen processes per page load.

**An index is now a function of its corpus.** Indexing one corpus twice used to
produce two different sets of vectors, because the checkpoint that makes a run
durable was timed rather than counted and split an embedding batch at a
different point each time. It counts files now, and `SEMLITH_CHECKPOINT_SECS` is
`SEMLITH_CHECKPOINT_FILES`. Neither was ever on the covered list — the variable
exists so a test need not wait thirty seconds — but a store built by 0.18.0 will
not be byte-identical to one built by 0.17.3 from the same corpus, and two built
by 0.18.0 will be. Nothing needs re-indexing: an existing store is read exactly
as before.

**Nothing else about the store changes.** `FORMAT_VERSION` does not move, and a
store written by 0.17.3 and one written by 0.18.0 are byte-compatible in both
directions. `tests/retrieval.rs` changed what it indexes — a snapshot of `src`,
`tests`, `docs` and `AGENTS.md` rather than the repository root — which moves
every number that harness reports; it is a test, not a surface, and the reason is
issue #88.

## 0.19.0

**One change alters what a store holds; everything else is a run that stops
giving up.** No command is removed, no argument changes meaning, and nothing
about ranking or scoring moves — a question answered by 0.18.0 is answered the
same way here. `FORMAT_VERSION` does not move either: a store written by 0.18.0
opens under 0.19.0, and a 0.18.0 binary opens a store 0.19.0 wrote, with no
table and no column added in either direction.

### The hidden-file rule now applies only to a path the caller named

**This is the change to know about, because it changes what a store holds.**
Through 0.18.0 any dotfile was refused, however semlith arrived at it. From
0.19.0 the rule splits on who chose the path. A dotfile the walk yielded is
indexed, because the walk only yielded it after the user's own `.gitignore`
whitelisted it — `dist/*` followed by `!dist/.gitkeep` — and refusing it as
hidden is the walk contradicting itself. A dotfile the caller named is still
refused as hidden: `semlith index ~/.npmrc`, `semlith_index` with that path and
`semlith add` all get the same answer they got before.

**What that means for an existing store.** The next index pass over a tree that
holds whitelisted dotfiles puts files into the store that were not there before.
Nothing is removed by this rule and no store changes until it is indexed again,
but a re-index is no longer guaranteed to produce the corpus 0.18.0 produced. If
that matters for a particular tree, `semlith scan` says what the store holds
that today's rules would refuse, and the deny-list below is what stops the
change reaching a credential.

**Every other rule applies to both.** The credential directories and the
credential names are checked whether a path was walked to or named, exactly as
they were.

### The credential deny-list is wider

`DENIED_NAMES` gains three environment-file patterns and eight per-user
credential dotfiles. `.env` and `.env.*` are gone as separate entries because
`.env*` subsumes them and also covers `.envrc`, `.env-local` and `.env.vault`;
`*.env` and `*.env.*` are new and cover `dev.env`, `production.env.local` and
the files a Docker `env_file` points at. `.npmrc`, `.netrc`, `.pypirc`,
`.pgpass`, `.htpasswd`, `.boto`, `.s3cfg` and `*.ppk` are new outright.

Those last eight were caught by the hidden-file rule alone until this release,
which is why widening the list is not a separate improvement but the half of the
change above that keeps it safe: without it, whitelisting a dotfile would put an
npm token into a store. [`docs/security.md`](security.md) has the whole list and
what each entry is for. `--include-secrets` is still the only way past it.

### A credential content scan refuses a file for what is inside it

New in this release, and the first rule semlith applies that a file's name
cannot decide. Every file's text is scanned before it is chunked, stored or
embedded, including the text a reader produced for a `.docx` or a notebook.
Images and binaries are never scanned; they were never text. A match refuses the
whole file, and the reason names the kind of credential and the line it sits on
and never a character of what was matched.

`--include-secrets` indexes such a file anyway and the run then reports how many
files the scan would have refused, so the flag is never silent about what it
did. The shapes are listed in [`docs/security.md`](security.md), along with the
caveat that matters most: the scan covers the shapes in that table and nothing
else.

### A refused file an earlier run indexed is evicted in the same pass

A rule that widens otherwise leaves every store indexed under the old rule still
holding what the new one refuses. So a path refused by the deny-list or by the
content scan has its rows and vectors removed on the spot, and the refusal line
says so. This is how a `dev.env` that 0.18.0 indexed leaves the store on the
first run under 0.19.0, and how a file that was clean when it was indexed and
has since gained a token leaves it. The file on disk is untouched.

### `semlith scan` is added, and there is deliberately no MCP tool for it

`semlith scan [STORE]` runs both halves of the decision over every file a store
already holds — the deny-list against the name, the content table against the
text the store is holding — and prints each file semlith would refuse today with
the rule, or with the kind of credential and the line. It exits non-zero while
anything is found, so it is usable as a check; `--forget` evicts what it found
and `--json` emits JSON. This is the path for a store indexed before this
release.

There is no `semlith_scan`. An agent is not the party that decides what a store
may hold, and a tool that evicts files is a tool that can be talked into
evicting files. The portal's half is a Scan section on the Privacy page, which
calls the same `Semlith::scan` the command does.

### A file that fails on its own content no longer ends the run

**A new per-file outcome, `failed`.** Through 0.18.0 an index run died on the
first file it could not read, so one truncated PNG in a tree of ten thousand
files ended the run and left the rest unindexed. A decoder that rejects an
image's bytes, a parser that cannot read a source file, and an entry the walk
itself could not read are now reported as `failed`, carrying the underlying
error's own message, and the run indexes the next file. Every failed path is
named with its reason by the `done` event, by the CLI's summary and by what
`semlith_index` returns — a count is a number somebody has to go and
investigate.

**A failure of the embedding batch is still fatal.** That is the model failing
rather than a file, and a run that carried on past it would be a run producing a
store with holes in it that nothing had said were there.

**The watcher survives a per-file failure too.** A save that failed to embed
used to come back as an error from the re-index and end the watcher thread for
that store, so every later save anywhere in that tree was silently never
indexed. It is now a line on that store's event feed and `watching` stays true.
An error that is not a file's — the store cannot be opened, the index cannot be
saved — still stops the watcher, and still says so.

### The event stream says why

**`IndexProgress` and the daemon's `file` event carry `why`**, a string for the
`skipped`, `refused` and `failed` outcomes and null for the rest, where the
outcome is the whole of what there is to say. The skip reasons are a closed set:
`empty`, `over 8 MiB`, `not a regular file`, `unreadable: <the operating
system's own message>`, `binary`, `no text in this document` and `not a
decodable image`. "Skipped" against two thousand files and nothing else is
indistinguishable from a run that lost them, which is the report this release
came out of.

**The `done` event gains three fields**: `failed`, an array of `{path, why}` in
the same shape as the existing `refused` array; `skipped_reasons`, an object of
`{"<kind>": <count>}`; and `elapsed_ms`, the daemon's own elapsed time for the
run. These are additive under the *Additive fields* rule above, and the daemon's
HTTP routes are [not a covered surface](#what-is-not-covered) in any case.

### A running daemon notices a store another process created

On every read of `/api/stores`, and on every by-name miss in the daemon's own
store lookup, the registry is re-read and any registered directory the daemon
does not have open is opened. So `semlith index ~/work/new-project` from a
second process while `semlith start` is running shows up on the Stores page and
answers over MCP without a restart; until 0.19.0 an agent asking for it by name
was told no such store was open until the daemon was restarted.

Reconciliation is triggered by the miss rather than by a timer, and both places
it runs are already the slow path, so nothing in the indexing loop pays for it.
A directory whose lock another process holds is left alone and listed as being
written, never forced, because the daemon must not become a second writer of one
store — the next read tries again, which is what makes a `semlith index` that is
still running appear by itself when it finishes. A directory the registry names
and the disk no longer has is listed as missing. Such a row carries an
`unopened` field saying which of the two it is.

### Paths are rendered plain everywhere

`semlith_symbol` and `semlith_path` returned Windows verbatim paths —
`\\?\C:\work\api\src\lock.rs` — which no editor opens and no shell completes.
They now render the way every other surface has since 0.17.1, as do the daemon's
`file` event path and the `refused` and `failed` paths on `done`. The store
still holds the verbatim form, because that is what makes a path longer than 260
characters work; only the text on its way out is plain. A Windows script that
matched a leading `\\?\` in a graph tool's reply stops matching, and one that
hands the printed path to an editor starts working.

### `pattern` gains two arguments

`semlith_pattern`, `semlith pattern` and `GET /api/pattern` take `path` — the
same repeatable glob filter the other surfaces take, which the MCP tool
previously accepted nowhere and ignored — and `offset`, which skips that many
matches so a listing the 200-match cap cut short can be continued. The
truncation line now names the offset that continues it rather than leaving a
caller to guess at a narrower pattern. Neither cap has moved, the order is
total, and two calls with the same offset return the same matches.

## 0.20.0

**One change breaks something a script could have been reading, and it is an
`/api/` route.** No CLI command is removed, no flag changes meaning, no MCP tool
or schema moves, and nothing about ranking or scoring changes — a question
answered by 0.19.0 is answered the same way here. `FORMAT_VERSION` does not move:
a store written by 0.19.0 opens under 0.20.0, a 0.19.0 binary opens a store
0.20.0 wrote, and no table or column is added in either direction.

### `POST /api/index` answers immediately instead of streaming

Through 0.19.0 this route held an HTTP worker open for the whole of a run and
streamed newline-delimited JSON down it. It now queues the work and returns:

```json
{"runs": [{"run": 4, "store": "api", "path": "/Users/you/work/api"}], "target": "each"}
```

`target` is `"each"` or `"store"`. A run that could not be queued appears in the
same array with `error` in place of `run`, so one refused path does not take the
ones beside it. On the `store` target a row carries `run` and `store` and no
`path`, because one run took every path.

**What breaks.** A script that read the NDJSON stream to follow a run gets a
single JSON object and no stream. **What replaces it.** `GET /api/index/runs`,
polled: the run is the daemon's now, so a caller reads it rather than holding it.
The reason for the change is the worker: eight of them serve the whole portal,
and a route that pins one for the length of an index run is a portal that stops
answering while it indexes.

**The NDJSON stream stays where it is still the right shape** — a forwarded
`semlith_index`, whose caller is blocking on the answer and never held a worker
here. Nothing about the MCP tool changes.

`POST /api/add` answers the same way and for the same reason, adding `fetched`
and `url` to the object. The fetch itself is still synchronous, because its
refusals are what the caller has to be told.

### New routes, a new target and a new action

| Route | What it answers |
|---|---|
| `GET /api/index/runs` | Every store's run with its queue position, the daemon-wide queue in submission order, how many runs are on, and the three settings with their source, the derived value, the sentence that derived it and the machine reading behind it. The machine is re-read per call, so the memory figure is memory free *now*. |
| `GET /api/index/log?store=<name>&after=<seq>` | That run's log lines after a cursor. A cursor rather than an offset, so two clients reading the same run through their own cursors each see every line exactly once. The daemon keeps the last 500. |
| `GET /api/projects?path=<dir>` | The git repositories directly under a directory — a `.git` directory *or* file, so a worktree and a submodule count — each with its name, canonical path and the store already covering it if any. Where none of the children is a repository, its plain subdirectories instead, with `repositories: false`. One level only. Confined to the user's home exactly as `/api/dirs` is. |
| `GET /api/changes` | Six monotonic integers, one per data domain: `stores`, `runs`, `clients`, `ledger`, `events`, `privacy`. The portal polls this once a second and refetches only what moved. It is not a server-sent stream because a stream would hold one of the eight workers per open tab. |
| `POST /api/index/settings` | Saves any of `runs_at_once`, `embed_threads` and `index_memory_mb`. A setting the environment fixes is refused with 409, naming the variable. |

`POST /api/index` takes `"store": "each"` — one store per path, each resolved
through the same function `semlith index <path>` resolves through, including the
numeric suffix for a second `api`. No store at all means `each` for more than one
path and the single-store behaviour of 0.19.0 for one.

`POST /api/index/control` gains `dequeue`, which takes a waiting run out of the
queue. It answers at once and undoes nothing, because nothing of it was embedded
— which is the whole difference from stopping a run that is going. `pause`,
`resume` and `stop` are unchanged.

**None of this is a covered surface.** The daemon's HTTP routes are
[not a contract](#what-is-not-covered) and never have been; this section exists
because a script may have been reading that stream anyway, and a break nobody was
told about is the same to whoever hits it.

### `semlith index --each` and `--projects`

`--each` puts each path in its own store, named and placed exactly as
`semlith index <path>` would name and place it. It is sequential — one process,
one embedder, one store at a time; a user who wants them in parallel runs the
daemon, which is what it is for. `--each` with `--name` is refused rather than
guessed at, because a name is a name for one store.

`--projects <FOLDER>` takes the paths from the repositories directly under that
folder, or from its plain subfolders where none of them is a repository, and
implies `--each`. It is the same discovery `/api/projects` does, from the same
function, so the portal's checklist and the terminal cannot disagree about what is
under a folder.

`semlith index` with neither flag does exactly what it did: several paths go into
one store.

### The three indexing settings, and where they are kept

The daemon reads this machine — logical cores, total memory, memory free now —
and derives how many runs may be on at once, how many embedder threads each gets,
and how much memory a store's index may use. Each is a setting rather than a
constant, and the order of precedence is: the environment, then what the portal
saved, then the derivation.

| Setting | Variable | Field in `settings.json` |
|---|---|---|
| Runs at once | `SEMLITH_INDEX_PARALLEL` (new) | `runs_at_once` |
| Embedder threads per writer | `SEMLITH_EMBED_THREADS` | `embed_threads` |
| Index memory per store | `SEMLITH_INDEX_MEMORY` | `index_memory_mb` |

An explicit variable wins, because it is an instruction from whoever started the
process, and the portal cannot overrule one. `~/.semlith/settings.json` is created
the first time a field is changed and never before; a home without it derives all
three. It is tool-written state like `registry.json`, not a configuration file —
see [what is not covered](#what-is-not-covered).

`SEMLITH_EMBED_THREADS` and `SEMLITH_INDEX_MEMORY` keep the meanings they had.
What changes is the value in force when neither is set: it is derived from the
machine rather than fixed, so a daemon on a large machine will use more threads
and more memory than 0.19.0 did, and one on a small machine will use fewer.
`semlith start` prints all three with their source at startup.

### Runs wait for each other now

Each store's writer used to take the next job on its own queue with nothing above
it, so eleven open stores meant eleven index runs at once whatever the machine
had. A run is now admitted only while fewer than runs-at-once are running, and
otherwise waits in one daemon-wide FIFO ordered by submission. The head is
admitted the moment a run finishes, stops or fails. The watcher's own re-embeds
bypass admission: they are small, they already interleave through slices, and
holding a file save behind eleven queued repositories would make the watcher
useless exactly when the machine is busy.

A daemon that ends with runs still queued drops them — a run's state was that
daemon's — and its next start names each of them on that store's event feed as
never started, along with any run that was going and what it had committed.
Without that a queued folder would leave no trace anywhere, and the way anybody
found out would be noticing the search results were thin.

### The run clock measures the run

The elapsed time an index run reports was measured around one *slice* of it. A
run hands the writer back to the watcher every 45 seconds and returns as a fresh
job, so the reading restarted from zero every 45 seconds and every surface
faithfully reported a run that had just begun. It now starts when the run is
submitted, spans every slice, stops while the run is held, and freezes at its
total when the run ends. Nothing about the field's name or type changes; the
number was wrong and is not any more.

## 0.22.0

### Search hits can carry a fifth badge, `definition`

`lists` on a hit is an array of the lists that found it, and it gains a value:
`definition`, on a chunk that defines the exact name an identifier-shaped query
typed. It is additive under the *Additive fields* rule — a reader that switches
on the four it knows and passes anything else through is unaffected, and a
reader that rejects an unknown value was already going to break the first time
a list was added.

The badge is not a fifth list in the fusion. The chunks that define the name are
lifted above the fused order rather than scored into it, which is why it carries
no weight and why a hit can carry `definition` alone. It appears only for a
query `shape_of` calls an identifier; a question-shaped query never produces
one, whatever it names.

**Why.** On the pinned corpus the harness measures, five identifier questions
had no satisfying span in the first eight results, and two of those definitions
ranked first in the keyword index by themselves. Reciprocal-rank fusion is
nearly flat across the first ranks — `weight / (60 + rank)` — so one
authoritative list placing a chunk first is outvoted by two vague lists placing
a different chunk fourth and fifth, plus the graph list derived from them. An
agent that already knows a term and is handed eight chunks that are not its
definition goes back to grep, which is the behaviour this release exists to
stop.

## 0.28.0

**Nothing that was covered breaks.** No command, flag, MCP tool or schema is
removed or changes meaning. `FORMAT_VERSION` does not move. A 0.27.x store opens
under 0.28.0, and a 0.27.x binary opens a store 0.28.0 wrote. What changes is
additive, apart from two behaviours a script may have depended on, described
below.

### A new command, a new flag and a new variable

`semlith accel` shows and sets the accelerator lanes, and `semlith doctor --gpu`
checks them. Both are in the table above. Before checking a lane, `doctor --gpu`
downloads the components the lane needs, as a run would. The command prints one
row per lane: the device, the variant, the lowest cosine of the 32 fixture
chunks against CPU fp32, and chunks/s. A lane with nothing to run on is printed
as `n/a` with the reason. `--json` returns the same rows as an array of objects
with the fields `lane`, `device`, `variant`, `cosine`, `chunks_per_s`, `passed`
and `reason`.

`SEMLITH_ACCEL` joins the three limit variables and takes precedence in the same
way:

| Setting | Variable | Field in `settings.json` |
|---|---|---|
| Which lanes embed | `SEMLITH_ACCEL` (new) | `accelerators`, an object with the optional booleans `cpu`, `gpu` and `cuda` |

A missing field means the default: CPU on, GPU on, CUDA off. A 0.27.x binary
reading a `settings.json` that contains `accelerators` ignores the key. The
file is still tool-written state and is [not covered](#what-is-not-covered).

### Two behaviours that changed

**Saved limits are applied.** In 0.27.0 `embed_threads` and `index_memory_mb`
were saved in `settings.json` and then not applied. In 0.28.0 they take effect:
threads at each writer's next batch, and the memory budget on every open index
at once. A value saved under 0.27.0 is therefore in force after the upgrade. A
saved `embed_threads` of 1 limits every writer to one thread. `POST
/api/index/settings` now replies with `applied`, a sentence giving the values
the engine runs with.

**Watcher catch-ups and large event batches are admitted.** Since 0.20.0 the
watcher's own work bypassed *runs at once*. A catch-up, which walks every root
when a store is opened, is now admitted like a run and appears in
`/api/index/runs`. So does an event batch of more than 32 files. Batches of 32
files or fewer still run straight away. A script that counted runs in that
route sees runs it did not start. They have a `kind` other than `run`.

### Route changes

The daemon's routes are [not a contract](#what-is-not-covered). They are listed
here because scripts read them anyway.

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

`held` and `pausing` are new values of a status field. A reader that switches
on the values it knows and passes anything else through is unaffected.

### `stats` gains a `variants` line, and the store gains one meta row

A store now keeps, in a `variants` row of its `meta` table, how many chunks each
variant of the model has embedded into it. The row is a JSON object such as
`{"fp16-webgpu": 2210, "int8-cpu": 1904}`. It counts what was embedded, so a
file that is later forgotten does not reduce it. `semlith stats` prints it as
`variants 1904 int8-cpu, 2210 fp16-webgpu` when the row exists. `semlith_stats`
prints the same per store, plus one `embedding lanes on:` line for the whole
answer. A store that nothing has embedded into since the upgrade has no row,
and neither line appears for it.

An older binary does not read the `meta` keys it does not know, so a 0.27.x
binary opening a store with this row ignores it. Its int8 queries search the
fp16 vectors in that store, which agree with int8 vectors of the same text at
cosine 0.987 (tested on the 2026-09-23 corpus). That is the only store change in
this release.

### The login service definition

`semlith setup` and `semlith upgrade` rewrite a login service registered by an
earlier release. On macOS the plist's `ProcessType` goes from `Background` to
`Standard`. On Windows the logon task is registered with `-Priority 5`. The
Linux unit is unchanged. `semlith doctor` prints `FAIL service priority` for a
definition that has not been rewritten. An older binary started by a rewritten
plist runs at normal priority the whole time. That is safe, but it does not
drop to background when idle. Running that version's `setup` restores its own
plist.

## 0.29.0

**Nothing that was covered changes.** No command, flag, MCP tool or schema
changes, `FORMAT_VERSION` does not move, and stores and `settings.json` are
untouched, so a 0.28.x binary and a 0.29.0 binary open each other's stores.
Two routes gain fields. The daemon's routes are
[not a contract](#what-is-not-covered); they are listed because scripts read
them anyway.

| Route | What changes |
|---|---|
| `GET /api/index/runs`, per run | `bytes` and `bytes_total`: the bytes of the files the run will open, done and in all, with a file being embedded counted in proportion to its chunks. `eta_ms`: milliseconds left at the bytes/s rate over the last 10 s of active time, null until 5 s of embedding has been seen and whenever the run is not `running`. `started_at` and `finished_at`: unix seconds, beside the `submitted` that was already there. `queued_ms`: milliseconds on the run's clock before it started. |
| `POST /api/store/delete` | Takes `{"stores": ["a", "b"]}` as well as `{"store": "a"}`. Each store in the list goes through the same removal as a single one, and a store that cannot be deleted does not stop the others. The list form answers `{deleted: [...], failed: [{store, error}], message}`: 200 when any store was deleted, and 409 with an `error` naming why when none was. The single form and its answer are unchanged. |

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
