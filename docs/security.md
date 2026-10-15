# Security model

What semlith refuses, what it downloads, and how you check each claim. It is
written for someone deciding whether to point an agent at their whole corpus.
To report a vulnerability, see [SECURITY.md](../SECURITY.md).

Every rule below has a row on the portal's **Privacy** page with the daemon's
own check of it, so you can read the check rather than the claim.

semlith runs as you, holds the text of everything you indexed, listens on a
loopback port, and hands an agent a credential. That raises three questions:

1. Can anything else on this machine act as you through semlith?
2. Can something you did not write decide what semlith answers with?
3. If the agent's key leaks, what does the holder get?

## Nothing else on this machine can act as you

**The session token is a header, not a cookie.** Every port on `localhost` is
the same site, so a cookie, even `SameSite=Strict`, would be attached to
requests from any other page served on `127.0.0.1` — a dev server, another
tool's dashboard. The token travels in a `Semlith-Token` header instead, which
only the page semlith served attaches and which cannot be set on an `<img>`, a
form or a stylesheet.

**A write has to prove where it came from.** Any request that is not a GET
needs `Sec-Fetch-Site: same-origin` (the browser's own statement, which script
cannot forge) or, from a client that sends no fetch metadata, a matching `Origin`
or none. It also needs a JSON content type, because the three types a
cross-origin form can post without permission are the three this refuses. Both
are checked before the token, so a page guessing at it cannot tell a right guess
from a wrong one.

**Guessing is slow.** The token is 32 bytes from the OS random source. A wrong
one is answered after a quarter of a second, doubling to four seconds once a
minute has carried more than twenty refusals. The delay runs on its own thread,
so a flood of guesses cannot stop the portal answering.

**Check it.** Open the portal, then your browser's developer tools:
`document.cookie` is empty. From a page on any other local port:

```js
await fetch("http://127.0.0.1:7365/api/rotate", { method: "POST" })
// 403, with an empty body
```

## Nothing you did not write decides what semlith answers with

**A store semlith did not create is not opened until you say so.** A `.semlith`
directory can arrive inside a repository you cloned: a corpus someone else
chose, answering your agent's questions, with a `daemon.json` that would choose
the port those questions travel through. Such a store is refused, with the two
ways forward named:

```sh
semlith trust ./.semlith   # keep it where it is
semlith adopt ./.semlith   # move it into ~/.semlith/stores
```

`--store` and `SEMLITH_STORE` still open anything: naming a store is an
instruction, not a discovery. Every store `semlith index` made is trusted by
living in the store home.

**A `daemon.json` is checked before it is followed.** It must be in a trusted
store, mode `0600`, owned by you, name a live process and carry a 64-hex-character
token. Otherwise semlith opens the store directly, as it would with no file.

**The store file is data, not a program.** SQLite can carry views and triggers
that run when a database is read. Every connection opens with `trusted_schema`
off and defensive mode on, and refuses writes until one of the three paths that
write asks for permission. From 0.41.0 a store outside the store home is also
put through SQLite's `quick_check` the first time it is opened, before anything
writes to it, and read with `cell_size_check` on; a store that fails is refused
by name.

**The weights are pinned.** Each model repository is fixed to a commit and each
file verified against a SHA-256 recorded in the source and in
[models.md](models.md). semlith also refuses a model cache another account owns
or can write to.

## What a leaked agent key gets

The agent key opens `/mcp` and nothing else: it cannot rotate a token, adopt a
store or start an upgrade. What it can read is bounded too.

**Read-only unless you say otherwise.** From 0.40.0 the key carries a scope,
kept in `~/.semlith/agent.scope`. `read`, the default and what every key from
before 0.40.0 became on its first start, lets an agent call the tools that only
read: search, brief, read, files, symbol, neighbors, impact, stats, languages,
report, pattern, path and trace. `semlith_index`, `semlith_add` and
`semlith_forget` answer a refusal that names how to grant them, and the refused
call is a ledger row. The check is on the call, not the tool list: a client can
call a tool it was never shown, and before 0.40.0 that was the whole of what
kept a read agent from writing. The same rule holds over `/mcp`, through
`semlith mcp` forwarding to the daemon, and in a `semlith mcp` running on its
own. The portal holds the session token and keeps the owner's rights.

```sh
semlith key                  # prints the scope
semlith key --scope write    # agents may index, add and forget too
semlith key --scope read     # back to read-only
```

The portal's Agents page has the same switch (Agent key ▸ Allow writes) and
`semlith doctor` prints the scope. An agent is steered by whatever it reads —
a web page, an issue, a file in a repository — so grant `write` only to agents
you trust with fetching URLs into a store and removing files from it.

**Indexing happens inside a boundary.** `semlith_index` and the portal's index
route accept a path only under the target store's registered roots, under the
store's own directory when it is a `.semlith` beside its corpus, or under your
home directory. `/etc`, another user's home, or a directory semlith was never
told about is refused by name, with the rule that refused it.

An agent's path outside those is refused before semlith looks at the disk, in
the same words whether or not it exists, so the tool cannot be used to map the
machine.

**Never a credential.** Whatever the boundary, nothing inside a `.git`, `.ssh`,
`.aws`, `.gnupg`, `.kube`, `.config/gcloud`, `.config/gh`, `.azure`, `.docker`,
`Library/Keychains`, `.password-store` or `.local/share/keyrings` directory is
indexed — wherever that directory sits, not only under your home (from 0.41.0),
and whatever a repository's ignore file whitelists: `!.git/` in a `.gitignore`
does not bring `.git/config` and the token in its remote into a store. Neither
is a file named `.env*`, `*.env`, `*.env.*`, `*.pem`,
`*.key`, `*.p12`, `*.pfx`, `*.jks`, `id_rsa*`, `id_ed25519*`, `*credentials*`,
`*secret*`, `*.tfstate`, `*.kdbx`, `.npmrc`, `.netrc`, `.pypirc`, `.pgpass`,
`.htpasswd`, `.boto`, `.s3cfg` or `*.ppk`. That is `filter::DENIED_NAMES` in
full. Each pattern matches the file name alone, case-insensitively, with `*`
standing for any run of characters; where the file lives does not matter.

- `.env*` covers `.env`, `.env.*`, `.envrc`, `.env-local` and `.env.vault`;
  `*.env` and `*.env.*` cover `dev.env`, `production.env.local` and the files a
  Docker `env_file` points at. `.env.example` is refused too, deliberately: a
  placeholder today is a real value after somebody fills it in.
- A dotfile the walk yields is indexed, because the walk yields it only after
  your own `.gitignore` whitelisted it (`dist/*` followed by `!dist/.gitkeep`).
  A dotfile you *name* is still refused as hidden, so `semlith index ~/.npmrc`
  is refused. Naming `.npmrc`, `.netrc` and the other per-user dotfiles in the
  list is what keeps a whitelisted dotfile from putting an npm token into a
  store.

`semlith index` on the command line keeps the deny-list but not the boundary:
the person typing it owns the machine. `--include-secrets` says you meant it.

**`semlith add` fetches from the public internet only.** Every hop is resolved
and refused if it lands on loopback, an RFC 1918 range, link-local
(`169.254.169.254` included), carrier-grade NAT or a unique local address, or on
an IPv6 address carrying one of those (mapped, NAT64, 6to4) or a Teredo tunnel.
From 0.41.0 the connection uses the addresses that were checked, so a name that
answers differently the second time it is asked cannot slip past, and a URL with
credentials in it is refused rather than sent or stored.
`SEMLITH_ADD_ALLOW_PRIVATE=1` opts back in for an intranet host, and
`semlith doctor` says when it is set. The other developer overrides
(`SEMLITH_ADD_ORIGIN`, `SEMLITH_PACK_*`, `SEMLITH_SERVICE_DIR`) do nothing in a
release build.

**The key stays in one file.** `~/.semlith/agent.key`, mode `0600`. A client
semlith registers launches `semlith mcp` as a subprocess, which reads the key
itself, so no configuration file semlith writes carries it. The HTTP stanzas in
[clients.md](clients.md) name `${SEMLITH_AGENT_KEY}`, which you export yourself
from `semlith key show`. The key never appears on a command line, so `ps` cannot
show it, and `/api/agents` does not return it: the portal fetches it once, when
you press Reveal. A rotated key stops working fifteen minutes later.

## What is inside a file, not only what it is called

A name says nothing about an AWS key pasted into `README.md`. So every file's
text is scanned before it is chunked, stored or embedded, including the text a
reader produced for a `.docx`, a PDF or a notebook. Images and binaries are not
scanned. A file refused this way never reaches the store.

### What the scan looks for

One table, `filter::SHAPES`; the verdict on each match (dummy or live, its
confidence, its mask) is `keyscan.rs`. Every row but one is anchored on the
credential's own prefix, because a prefix is the issuer declaring what the
string is:

| Credential | Prefix |
|---|---|
| Anthropic API key | `sk-ant-` |
| OpenAI API key | `sk-` |
| AWS access key id | `AKIA`, `ASIA` |
| GitHub token | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` |
| GitHub fine-grained token | `github_pat_` |
| Slack token | `xoxa-`, `xoxb-`, `xoxp-`, `xoxr-`, `xoxs-` |
| Stripe key | `sk_live_`, `rk_live_`, `sk_test_`, `rk_test_` |
| Google API key | `AIza` |
| Twilio API key | `SK` followed by 32 hex characters |
| SendGrid API key | `SG.` |
| npm token | `npm_` |
| semlith agent key | `sml_` |
| Private key | a `-----BEGIN … PRIVATE KEY-----` block |
| JSON web token | `eyJ`, in three dot-separated parts |

Stripe test keys are refused too: a table with an exception is wrong the day
someone pastes a live key into `test.md`. Each row carries an example it must
match (a declared test dummy, so the table's own file is not refused) and a near
miss it must not. `tests/scan.rs` walks the table, builds a live-shaped value
for each row at run time, and fails the build if the example is not a dummy, the
built value is not refused, or the near miss matches. No live-looking literal is
in the source.

**The one rule without a prefix needs two things at once:** an assignment whose
left side is a key-like name — `api_key`, `secret`, `token`, `password`,
`passwd`, `auth`, or one ending `_KEY`, `_SECRET` or `_TOKEN`, inside any
identifier — and whose right side is at least twenty characters of high entropy.
The value may be quoted, or unquoted on a line of its own as a `.env`, a shell
`export` or YAML writes it (`AWS_SECRET_ACCESS_KEY=…`). This catches an AWS
secret access key, which has no prefix. The name alone would refuse
`password = "hunter2"` and `PASSWORD=changeme`; the entropy alone would refuse
every base64 fixture and lockfile hash. Together they are narrow enough to be on
by default.

Two shapes are never a match for that rule:

- a template reference (`${SEMLITH_AGENT_KEY}`, `{{token}}`, `<your-key-here>`)
  or a bare `SCREAMING_SNAKE` variable name;
- a value crossing a line. An unquoted value is read only when it is the whole
  rest of its line and made of characters a key is made of, so
  `let token = compute_token(input);` is not a match.

**A refusal never quotes the credential.** It names the kind of thing matched and
its line. No character of the match is written to the event, the CLI's output,
the store, a log or the portal. Where a match has to be shown for a person to
decide, it is masked to the issuer's prefix and the last four characters
(`ghp_…Xa9Q`).

### Test dummies

A match is a test dummy, and does not refuse its file, when one of three
declared rules says so:

- (a) it is on a short list of published documentation examples, such as AWS's
  `AKIAIOSFODNN7EXAMPLE` and `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`;
- (b) the token's body — after the issuer's prefix — contains `EXAMPLE`,
  `FAKE`, `DUMMY`, `PLACEHOLDER`, `REDACTED` or a run of eight or more `X`, or is
  one character repeated. Only the body counts: a comment saying "test" beside a
  live key changes nothing;
- (c) it is a private-key `BEGIN` line with no key under it: fewer than 40
  base64 characters before the `END` line or the end of the text.

The rules are declared, not guessed from entropy, because a rule that let
through anything random enough would index the one real key that was not.
**One live-looking match anywhere still refuses the whole file**, dummies or not,
because a companion secret (an AWS secret access key under a name the assignment
rule does not know) cannot be detected by prefix. `semlith index` says how many
files it indexed holding only dummies, and `semlith refused` lists them under
"let through as test dummies". To refuse one anyway, run `semlith refused refuse
<path>` or press **Refuse instead** on Files ▸ Decisions.

### Every refused file is listed

Each store keeps a `refusals` table, written by `semlith index`, the watcher and
the daemon's catch-up, and read by `semlith refused`, `/api/refused` and the
portal (the Index page's scan panel, and the Files page's Decisions tab and
tree). A row is one of five classes:

| Class | What | Can a person accept it? |
|---|---|---|
| (a) content scan | a secret-shaped value | yes |
| (b) credential file | a name or folder on the deny-list | never; only `--include-secrets` on the command line indexes one |
| (c) policy limit | over the size cap, or inside a pruned generated folder such as `node_modules` (shown once per folder) | yes |
| (d) not indexable | empty, binary, no text, unreadable, not a regular file, not a decodable image | no action: accepting cannot make it indexable |
| (e) your own exclusions | `.gitignore`, `.semlithignore`, or outside the store's roots | change the rule, not the file |

**A secret row carries a confidence from 0 % to 100 %**: an estimate of how
likely the match is a real, working secret. It is built from declared signals,
each shown beside the number: whether the value has its provider's documented
length and characters; a valid CRC32 checksum on a GitHub or npm token (at least
90 %) or an invalid one (at most 10 %); how random the body is against the
provider's generator, lower for a repeated, sequential or dictionary body; a
companion, such as a 40-character secret access key within five lines of an AWS
key id or a private-key body that decodes to a DER sequence; the location, lower
under `tests/`, `fixtures/`, `examples/` or `docs/`, in Markdown or in a test
function, higher in a configuration, CI or deploy file; and a JSON web token
whose `exp` has passed. **It is an estimate and decides nothing**; only the
dummy rules decide.

### Accepting a refused file

**A person may accept one refused file at a time, never an agent and never in
bulk.** From the portal's scan panel after a scan, or with `semlith refused
accept <path>`, one path per call, after the file, each masked match, its line
and its confidence have been shown and the file's name typed (or "I have
reviewed this file" ticked). There is no select-all and no glob, and credential
files are never offered. `semlith refused revoke` or the Files ▸ Decisions tab
undoes a decision, one file at a time.

- **Accept with redaction** replaces each detected value with
  `[REDACTED:<provider> <kind>]` before anything is chunked, embedded or written
  to the full-text index, keeping line numbers. The secret never enters the
  store. Redaction covers only what the scanner detected, and the confirmation
  says so.
- **Accept as-is** indexes the file's full text, values included.

The accept routes need the portal's session token; the agent key opens only
`/mcp`, and no MCP tool accepts. An acceptance is stored in the store's database
as the path, the class, the mode, the confidence at the time and a salted
blake3 fingerprint of each accepted match, never the value. Each accept and
revoke writes a hash-chained ledger row with the same fields.

**An edit never lets a new secret through.** Every later pass scans the file
again. If every live match's fingerprint is one the person accepted, the file is
indexed in their mode, with redaction applied to the current text. If a new
match appears, the file is refused again, its old copy is evicted, and it
returns to the list marked `new match since accepted, line N`. A read of an
accepted-redacted file from disk (`semlith_read` on a file edited since
indexing) applies the same redaction.

### When the scan runs

**Before anything is embedded.** Every index run opens with a scan phase that
needs no model: it walks, stats, reads and hashes each file, sorts it into the
classes above, and shows the plan. The portal's **Scan** holds every run after
its scan and embeds nothing until **Start indexing** is pressed; an undecided
file stays refused. `semlith index` in a terminal stops for review only when
something is reviewable. An agent's run, the watcher and a piped `semlith index`
never wait, and the MCP reply says how many files await review.

**A file today's rules refuse leaves the store on the next pass.** A path refused
by the deny-list or by the content scan has its rows and vectors removed in the
same run, and the refusal line says so. The file on disk is untouched. From
0.40.0 everything derived from its text goes in that run too: the keyword
index's terms (FTS5 secure delete), its full-precision vectors in `exact.f32`
(zeroed), its entries in the vector cache, and the freed pages of the store's
database (vacuumed before the run ends).

**`semlith scan` checks a store indexed earlier.** It runs both halves over every
file a store holds — the deny-list against the name, the content table against
the stored text — and prints each file semlith would refuse today, with the
rule or the kind and line. It exits non-zero while anything is found, so it
works as a check in a script; `--forget` evicts what it found. The Privacy page
has the same check, from the same function, with a button per row. There is no
MCP tool for it: an agent does not decide what a store may hold.

**The scan is a net, not a guarantee.** It refuses the shapes in the table and
nothing else. A credential with no issuer prefix, a format only your service
uses, one split across two lines, or one base64-encoded inside something else
does not match. Do not index a directory you otherwise would not because the
scan exists, and run `semlith scan` afterwards.

## What is yours alone on disk

`~/.semlith`, every store directory, the model cache and `~/.semlith/bin` are
created `0700` and narrowed back to `0700` on every open if something loosened
them. From 0.41.0 every file and directory semlith creates is its user's alone
by construction, through a `077` umask: `store.db` and SQLite's own WAL files,
the vector shards, `exact.f32`, `index.lock`, logs, the parse journal,
`registry.json`, `agent.key`, `daemon.json`, a scheduled report. Nothing is
widened: a directory you set to `0500` stays `0500`. `semlith stats` and the
Stores page report a store other users can read.

The session token is in `daemon.json` and nowhere else on disk. A store kept
outside the store home, such as a `.semlith` beside its corpus in a synced
folder, has its `daemon.json` under `~/.semlith/run/` instead of beside it, and
one an older daemon left there is removed. A daemon running as a service prints
its portal URL without the token, because its output is a log file; `semlith
start` prints the whole URL of a running daemon.

**The ledger chain is keyed.** Each store's retrieval ledger is hash-chained,
and from 0.41.0 the chain is keyed with a per-store key in `ledger.key` beside
the database (`0600`). Someone who can rewrite `store.db` alone, or who has a
copy of it, cannot recompute the chain after an edit. Rows written before stay
verifiable under the old formula.

```sh
stat -f '%Sp %N' ~/.semlith ~/.semlith/agent.key   # macOS
stat -c '%A %n' ~/.semlith ~/.semlith/agent.key    # Linux
```

## What leaves the machine when you search

Nothing, until you connect a cloud org with `semlith cloud connect`. After that,
a search that names no store, or names a remote one, also asks every connected
org's stores, and sends them the query and its filters. `semlith search
--local`, `local: true` on `semlith_search` and *This machine only* on the
portal's Search page keep one search here. A tool other than search goes to an
org only when it names one of its stores.

## What semlith downloads, and when

Every download is pinned by URL and SHA-256 ([models.md](models.md) has each
digest) and checked while it streams; a file whose digest does not match is
deleted and refused. Nothing is fetched on a timer or at startup without a
reason below. The Privacy page lists every download the binary can make, with
its source, size, when it happens and whether it is cached.

| What | From | When |
|---|---|---|
| The embedding model (~52 MB) | `huggingface.co` | the first index; `semlith setup` pre-fetches it |
| The rescoring model | `huggingface.co` | `semlith setup`, beside the embedding model |
| CLIP, vision and text (335 MB and 244 MB) | `huggingface.co` | the first image a store indexes; never at start. Checked against its pinned digests before ONNX Runtime loads it |
| The WebGPU plugin and the fp16 model | `files.pythonhosted.org`, `huggingface.co` | the first run with the GPU lane on and a hardware GPU found; never for a software renderer |
| The CUDA pack (1.89 GB) | `github.com`, `files.pythonhosted.org` | Linux only, after `semlith accel on cuda` or the page's switch, which states the size first |
| The Core ML pack (148 MB) | this repository's GitHub release | `semlith setup` on Apple silicon, or `semlith accel on ane` |
| llama.cpp, TensorRT for RTX, OpenVINO | GitHub releases, `files.pythonhosted.org` | only when you turn on that experimental lane |
| ONNX Runtime | `cdn.pyke.io` (pinned by `ort-sys`) | at build time, for `cargo install`, source builds and the macOS and Windows release archives, which link it in |
| `semlith add <url>` | that URL | when you run it |
| `semlith upgrade` | `github.com` | when you run it |
| Semlith Cloud | the host you signed in to | only after `semlith cloud login` |

**Releases are signed.** From v0.40.0 every release archive and its
`SHA256SUMS` carry an ed25519 signature made by the release workflow with the
project's key (an OpenSSH `SSHSIG`, namespace `file`, beside each file as
`.sig`). The public half is built into the binary — `semlith --version` prints
its id, `SHA256:a5oJmrakIwDHEWTDWqWe51Ttt6m2tQ0dsGzIqyxDuxs` — and into
`install.sh` and `install.ps1`. `semlith upgrade` verifies the signature before
it writes or unpacks anything, and both installers verify it with
`ssh-keygen -Y verify` before the checksum; a missing, unparseable, untrusted or
wrong signature stops all three. A checksum from the same release only proves
the archive is the one the release names; the signature proves who made the
release. If the key ever changes, one release trusts both the old and the new
key, so an older binary can still upgrade to it.

`--airgap` refuses every download unless it is already in the model cache, and
names what it refused. An accelerator lane runs in its own worker process, so a
driver crash ends that worker and its lane, not the daemon. The llama.cpp server
listens on `127.0.0.1` with a key only its worker holds, and dies with the
worker.

## A file that crashes a parser

Every reader of a file's own bytes — the tree-sitter grammars, PDF, the Office
and OpenDocument readers, mail, notebooks and an image decode check — runs in a
parse worker: this same binary started again as `semlith __parse-worker`,
spoken to over pipes, pooled so there is one per thread that parses. A worker
has a deadline (30 seconds, and one more per MiB of input) and a memory limit
(2 GiB of allocations, `SEMLITH_PARSE_MEMORY_MB` to change it; on Linux and
Windows the operating system enforces it for C code too). Before 0.40.0 a
606-byte Markdown file of nested blockquotes aborted the daemon through a C
assertion in tree-sitter-md — tree-sitter-yaml has the same one — and under
launchd or systemd the service then crash-looped for as long as the file sat in
a watched folder.

Now an abort, a stack overflow, running out of memory or out of time costs one
worker. The file is refused with the reason (`parser crashed: SIGABRT`,
`parser timed out after 30 s`, `parser ran past its memory limit`), listed by
`semlith refused`, on the portal's Files ▸ Decisions as *parser crashed*, and by
`semlith doctor`. Every store keeps a journal of what it handed a parser
(`logs/parse-journal` in the store), so a refused version of a file is never
parsed again, across restarts; a file that was mid-parse when the whole process
died twice is refused the same way. Changing the file clears it, and *Try
again* (or `semlith refused accept <path> --as-is`) gives it one more worker.

Running out of time while making a file's outline costs the outline only: the
text is read again on its own and indexed without one, as a parse that passes
tree-sitter's own two-second limit always was. A file is refused for time only
when its text cannot be read in time.

The Markdown and YAML grammars are also given at most 128 levels of nesting —
both abort at about 260. A file nested deeper keeps its text in the store and
has no outline.

## Work a file can make semlith do

One file costs seconds whatever it holds. Before 0.41.0 the secret scan and the
symbol graph compared every match or definition with every other, so a planted
8 MiB file of key-shaped strings or one-line functions was CPU-minutes per file;
both are linear now, and the 8 MiB worst cases take under 5 seconds on an M1
(`tests/security/s18.rs`). `semlith_pattern` and exact search stop after ten
seconds and say they were cut short, as they do at 200 matches.

An export cannot run anything. A CSV cell that a spreadsheet would read as a
formula (`=`, `+`, `-`, `@`, a tab or a carriage return first) is prefixed with
`'`, and a Markdown export shows an agent's query as text rather than as HTML.

## Reading what was indexed, later

A file is opened once, without following a symbolic link (`O_NOFOLLOW`; a
reparse point on Windows), without blocking on a FIFO, and checked to be a
regular file on the open handle before a capped read. That holds for indexing,
the scan before a review, accepting a refused file, `semlith cloud push`, the
portal's image view and `read`'s fallback, which serves a stale span's current lines: a stored
path that no longer resolves to itself, or that the deny-list names, is not
read, and the span says the file was replaced since it was indexed. Before
0.40.0 a file swapped for a link to `~/.kube/config` after indexing was served
to the agent that read it.

A repository is untrusted the same way. Every `git` semlith runs ignores the
system config and has `core.fsmonitor`, hooks, `gpg.program` and signature
display switched off, so a `.git/config` that arrived with an archive cannot
make indexing run a program.

## What is not covered

**The store is not encrypted.** It holds the plain text of everything you
indexed. The file modes above are the boundary semlith draws; encryption at rest
is a separate decision semlith has not made.

**The parse worker is not a sandbox.** It runs as you, with your files, and can
open what you can. It bounds what a crashing or runaway parser costs; it does
not stop a parser bug that reads or writes files. On macOS the memory limit
covers Rust's allocations only, because Darwin does not enforce a lower
`RLIMIT_AS`; the deadline bounds the rest. The embedding model and ONNX Runtime
run in the daemon.

**Downgrading below 0.14.0 gives back what was refused.** A 0.13.0 binary opens
every store 0.14.0 and later wrote, ignores the trusted list, and reads a `0700`
directory as its owner. It restores the cookie session and the unbounded index
tool.

## Proof that stays

A rule that held once is not a rule that holds next release, so these run on
every pull request:

- `tests/security.rs`, one test per finding of the 2026-10-14 audit, each of
  which failed on the release before its fix;
- `cargo deny` (advisories, licences, sources; `deny.toml`) and `cargo audit`
  with a freshly fetched advisory database, as required checks.

And on a schedule, weekly and on demand: `cargo-fuzz` targets for the HTTP
request head, the Markdown and code chunkers and every format reader (Office
and OpenDocument, PDF, mail, notebooks, HTML and RTF, images), an hour each
(`.github/workflows/fuzz.yml`). Releases go to crates.io through Trusted
Publishing, so no long-lived crates.io token exists.

### Controls by release

| Release | Controls |
|---|---|
| 0.14.0 | header token, fetch-metadata check, trusted stores, pinned weights, index boundary, deny-list |
| 0.40.0 | parse worker and poison journal, read-only agent keys, no global MCP lock, link-safe reads, signed releases, git without repository config, llama key off argv, config writes keep modes, refused secrets scrubbed, HTTP head deadlines |
| 0.41.0 | `.git` and credential directories refused anywhere, token out of logs, digests checked before load, overrides out of release builds, quoting, linear scan and graph, bounded pattern and grep, `add` connects to the checked address, refused repeated headers, owner-only files, foreign-store check, keyed ledger, local-only search, export escaping, CI gate, fuzzing, Trusted Publishing |

Every rule on this page except the content scan closes a finding from the
security audit of 0.13.0 or of 0.39.0, and each shipped with the test that would
have caught it. `CHANGELOG.md` lists them by finding id;
[compatibility.md](compatibility.md) records the four that broke something 0.13.0
did, with the way back for each.
