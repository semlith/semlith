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
write asks for permission.

**The weights are pinned.** Each model repository is fixed to a commit and each
file verified against a SHA-256 recorded in the source and in
[models.md](models.md). semlith also refuses a model cache another account owns
or can write to.

## What a leaked agent key gets

The agent key opens `/mcp` and nothing else: it cannot rotate a token, adopt a
store or start an upgrade. What it can read is bounded too.

**Indexing happens inside a boundary.** `semlith_index` and the portal's index
route accept a path only under the target store's registered roots, under the
store's own directory when it is a `.semlith` beside its corpus, or under your
home directory. `/etc`, another user's home, or a directory semlith was never
told about is refused by name, with the rule that refused it.

**Never a credential.** Whatever the boundary, no path under `~/.ssh`, `~/.aws`,
`~/.gnupg`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, `~/.docker`,
`~/Library/Keychains`, `~/.password-store` or `~/.local/share/keyrings` is
indexed, and neither is a file named `.env*`, `*.env`, `*.env.*`, `*.pem`,
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
(`169.254.169.254` included), carrier-grade NAT or a unique local address.
`SEMLITH_ADD_ALLOW_PRIVATE=1` opts back in for an intranet host.

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
same run, and the refusal line says so. The file on disk is untouched.

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
them. `store.db`, the vector shards, `registry.json`, `agent.key` and
`daemon.json` are `0600`. Nothing is widened: a directory you set to `0500`
stays `0500`. `semlith stats` and the Stores page report a store other users can
read.

```sh
stat -f '%Sp %N' ~/.semlith ~/.semlith/agent.key   # macOS
stat -c '%A %n' ~/.semlith ~/.semlith/agent.key    # Linux
```

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
| CLIP, vision and text (335 MB and 244 MB) | `huggingface.co` | the first image a store indexes; never at start |
| The WebGPU plugin and the fp16 model | `files.pythonhosted.org`, `huggingface.co` | the first run with the GPU lane on and a hardware GPU found; never for a software renderer |
| The CUDA pack (1.89 GB) | `github.com`, `files.pythonhosted.org` | Linux only, after `semlith accel on cuda` or the page's switch, which states the size first |
| The Core ML pack (148 MB) | this repository's GitHub release | `semlith setup` on Apple silicon, or `semlith accel on ane` |
| llama.cpp, TensorRT for RTX, OpenVINO | GitHub releases, `files.pythonhosted.org` | only when you turn on that experimental lane |
| `semlith add <url>` | that URL | when you run it |
| `semlith upgrade` | `github.com` | when you run it |
| Semlith Cloud | the host you signed in to | only after `semlith cloud login` |

`--airgap` refuses every download unless it is already in the model cache, and
names what it refused. An accelerator lane runs in its own worker process, so a
driver crash ends that worker and its lane, not the daemon. The llama.cpp server
listens on `127.0.0.1` with a key only its worker holds, and dies with the
worker.

## What is not covered

**The store is not encrypted.** It holds the plain text of everything you
indexed. The file modes above are the boundary semlith draws; encryption at rest
is a separate decision semlith has not made.

**The parsers run in this process.** An image past 64 million pixels is refused
before a decoder allocates for it, a tree-sitter parse gives up after two
seconds, and a file is read once through a capped reader — but a document you
index is still parsed by code running as you. The parsers are not sandboxed.

**Downgrading below 0.14.0 gives back what was refused.** A 0.13.0 binary opens
every store 0.14.0 and later wrote, ignores the trusted list, and reads a `0700`
directory as its owner. It restores the cookie session and the unbounded index
tool.

Every rule on this page except the content scan closes a finding from the
security audit of 0.13.0, and each shipped with the test that would have caught
it. `CHANGELOG.md` lists them by finding id;
[compatibility.md](compatibility.md) records the four that broke something 0.13.0
did, with the way back for each.
