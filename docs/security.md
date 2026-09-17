# What semlith refuses, and how you check it

semlith's claim is that nothing leaves your machine and nothing on your machine
can reach it without your say-so. The first half has been checkable since 0.9.0:
bind loopback, run `tcpdump`, watch nothing happen. This page is the second
half, which 0.14.0 is mostly about.

It is written for somebody deciding whether to point an agent at their whole
corpus. `SECURITY.md` is where to report a problem; this is what the product
does. Every rule below has a row on the portal's **Privacy** page with the
daemon's own check of it, so you can read the reading rather than the claim.

## The shape of the problem

semlith runs as you. It holds the text of everything you indexed, it listens on
a port, and it hands an agent a credential that lives in a configuration file.
That makes three questions worth asking, and 0.14.0 answers them one at a time:

1. Can anything else on this machine act as me through semlith?
2. Can something I did not write decide what semlith answers with?
3. If the agent's key leaks, what does the person holding it get?

## Nothing else on this machine can act as you

**The session token is a header, not a cookie.** Every port on `localhost` is
the same site, so `SameSite=Strict` never separated semlith from a page served
by anything else on `127.0.0.1` — a development server, another tool's
dashboard, a page you opened once. A page on any of those could send a request
and the browser would attach the cookie. From 0.14.0 the token travels in a
`Semlith-Token` header, which a browser attaches only for the page semlith
served, and which cannot be set at all on an `<img>`, a form or a stylesheet.

**A write has to prove where it came from.** Anything that is not a GET needs
`Sec-Fetch-Site: same-origin` — the browser's own statement about the request,
which script cannot forge — or, from a client that sends no fetch metadata at
all, an `Origin` that matches or none. It also needs a JSON content type,
because the three types a cross-origin form can post without asking permission
are exactly the three this refuses. Both are checked before the token, so a
page guessing at it cannot tell a right guess from a wrong one.

**Guessing is slow.** The token is 32 bytes from the OS random source. A wrong
one is answered after a quarter of a second, doubling to four seconds once a
minute has carried more than twenty refusals — and the delay is held by a thread
of its own, so a flood of guesses cannot stop the portal answering.

**What you can check.** Open the portal, then open your browser's developer
tools: `document.cookie` is empty. Then, from a page on any other local port:

```js
await fetch("http://127.0.0.1:7365/api/rotate", { method: "POST" })
// 403, with an empty body
```

## Nothing you did not write decides what semlith answers with

**A store semlith did not create is not opened until you say so.** A `.semlith`
directory can arrive inside a repository you cloned. A store is what semlith
answers *from*, so a cloned one is a corpus somebody else chose, answering the
questions your agent asks — and the `daemon.json` beside it would choose the
port those questions travel through. From 0.14.0 such a store is refused, with
the two ways forward named:

```sh
semlith trust ./.semlith   # keep it where it is
semlith adopt ./.semlith   # move it into ~/.semlith/stores
```

`--store` and `SEMLITH_STORE` still open anything: naming a store is an
instruction, not a discovery. Every store `semlith index` made is trusted by
living in the store home.

**A `daemon.json` is checked before it is followed.** It has to be in a trusted
store, mode `0600`, owned by you, naming a process that is alive, carrying a
token that is 64 hex characters. Anything else and semlith opens the store
directly, which is what a missing file has always meant.

**The store file is data, not a program.** SQLite can carry views and triggers
that run when a database is merely read. Every connection opens with
`trusted_schema` off and defensive mode on, and refuses writes until one of the
three paths that write asks for permission.

**The weights are pinned.** Each model repository is fixed to a commit and each
file verified against a SHA-256 recorded in the source and in
[`docs/models.md`](models.md). A model that changed under you would change what
your corpus means without changing anything you can see. semlith also refuses a
model cache another account owns or can write to.

## What a leaked agent key gets

The agent key opens `/mcp` and nothing else — it cannot rotate a token, adopt a
store or start an upgrade. From 0.14.0 it is also bounded in what it can read.

**Indexing happens inside a boundary.** `semlith_index` and the portal's index
route accept a path only under the target store's registered roots, under the
store's own directory when it is a `.semlith` sitting beside its corpus, or under
your home directory. `/etc`, another user's home, a directory nobody told semlith
about: refused, by name, with the rule that refused it.

**And never a credential.** Whatever the boundary, no path under `~/.ssh`,
`~/.aws`, `~/.gnupg`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, `~/.docker`,
`~/Library/Keychains`, `~/.password-store` or `~/.local/share/keyrings` is
indexed, and neither is a file named `.env*`, `*.env`, `*.env.*`, `*.pem`,
`*.key`, `*.p12`, `*.pfx`, `*.jks`, `id_rsa*`, `id_ed25519*`, `*credentials*`,
`*secret*`, `*.tfstate`, `*.kdbx`, `.npmrc`, `.netrc`, `.pypirc`, `.pgpass`,
`.htpasswd`, `.boto`, `.s3cfg` or `*.ppk`. That is `filter::DENIED_NAMES` in
full. Each pattern is matched against the file name alone, case-insensitively,
with `*` standing for any run of characters — these are not rules about where a
file lives, because a `.env` in a repository is the same kind of thing as a
`.env` in a home directory.

**Three of those cover every shape of environment file rather than the two that
are conventional.** `.env*` subsumes `.env` and `.env.*` and also catches
`.envrc`, `.env-local` and `.env.vault`; `*.env` and `*.env.*` catch `dev.env`,
`production.env.local` and the files a Docker `env_file` points at. `.env.example`
is refused along with the rest, and that is deliberate rather than an oversight:
the name says what the file holds, and a placeholder today is a real value after
somebody fills it in and forgets which copy they edited.

**The eight per-user dotfiles at the end of that list are new in 0.19.0**, and
they are there because something else changed. Until then the hidden-file rule
caught `.npmrc` and its relatives on its own, by refusing every dotfile. From
0.19.0 a dotfile the walk yielded is indexed, because the walk only yielded it
after your own `.gitignore` whitelisted it — `dist/*` followed by
`!dist/.gitkeep` — and refusing it as hidden is the walk contradicting itself. A
dotfile you *named* is still refused as hidden, so `semlith index ~/.npmrc` is
answered the way it always was. Naming those eight is what keeps the first half
of that change from putting an npm token into a store.

`semlith index` on the command line keeps the deny-list and loses the boundary:
the person typing it owns the machine. `--include-secrets` is how you say you
meant it.

**`semlith add` fetches from the internet, and only the internet.** Every hop is
resolved and refused if it lands on loopback, an RFC 1918 range, link-local
(`169.254.169.254` included), carrier-grade NAT or a unique local address.
`SEMLITH_ADD_ALLOW_PRIVATE=1` opts back in for an intranet host.

**The key stops travelling.** It is in one file, mode `0600`. No client
configuration carries it: every stanza names `${SEMLITH_AGENT_KEY}`, which
`semlith setup`'s shell block exports by reading that file at shell start. It
never appears on a command line, so `ps` cannot show it, and `/api/agents` does
not return it — the portal fetches it once, when you press Reveal. A rotated key
stops working fifteen minutes later.

## What is inside a file, not only what it is called

A deny-list decides from a name, and an AWS key pasted into `README.md` has a
name that says nothing. From 0.19.0 every file's text is scanned before it is
chunked, stored or embedded — including the text a reader produced for a
`.docx`, a PDF or a notebook, which is how a key in the body of a document is
caught rather than only one in a file called `.env`. Images and binaries are
never scanned; they were never text. A file semlith refuses this way is a file
whose contents never reach the store at all.

**What it looks for** is one table, `filter::SHAPES`, and it is short on
purpose. Every row but one is anchored on the credential's own prefix, because a
prefix is the issuer declaring what the string is, and that is what makes a
string safe to refuse on sight: Anthropic API keys (`sk-ant-`), OpenAI API keys
(`sk-`), AWS access key ids (`AKIA` or `ASIA`), GitHub tokens (`ghp_`, `gho_`,
`ghu_`, `ghs_`, `ghr_`), GitHub fine-grained tokens (`github_pat_`), Slack
tokens (`xoxa-`, `xoxb-`, `xoxp-`, `xoxr-`, `xoxs-`), Stripe keys (`sk_live_`,
`rk_live_`, `sk_test_` and `rk_test_` — a test key is not a credential in the
sense a live one is, and it is refused anyway, because a table with an
exception in it is wrong the day somebody pastes a live key into a file called
`test.md`), Google API keys (`AIza`), Twilio API keys (`SK`
followed by 32 hex characters), SendGrid API keys (`SG.`), npm tokens (`npm_`),
semlith's own agent key (`sml_`), a `-----BEGIN … PRIVATE KEY-----` block, and a
JSON web token. Each row carries an example it must match and a near miss it
must not, and `tests/scan.rs` walks the table and fails the build on either — so
a typo in a pattern is a red build rather than a credential in a store.

**The one rule with no prefix needs two things at once.** An assignment whose
left side is a key-like name — `api_key`, `secret`, `token`, `password`,
`passwd`, `auth`, in any surrounding identifier — and whose quoted right side is
at least twenty characters of high entropy. Both halves are necessary. The name
alone refuses `password = "hunter2"`, which is not a credential worth refusing a
file for. The entropy alone refuses every base64 fixture and every lockfile hash
in a normal corpus. Together they are narrow enough to be on by default.

Two shapes are excluded from that rule, both of them things a credential is
never written as. A value that is a template reference — `${SEMLITH_AGENT_KEY}`,
`{{token}}`, `<your-key-here>` — or a bare `SCREAMING_SNAKE` variable name is a
placeholder, and refusing a file for carrying the documentation of how not to
write a key down is the rule working against itself. And the value may not cross
a line: without that, a `token=` on one line and a quote two lines later match as
one string whose contents are the code in between. Both came out of the
false-positive audit this rule was measured by, which found each of them in a
real tree.

**The reason never quotes the credential.** A refusal says what kind of thing
matched and the line it sat on, and no character of the matched text is written
anywhere semlith writes: not in the event, not in the CLI's output, not in the
store, not in a log, not on the portal. A scan that prints the secret it found
has moved the secret rather than refused it.

**There is no allow-list of known-fake values.** A documentation page quoting
AWS's own `AKIAIOSFODNN7EXAMPLE` is refused like any other match. A second table
of values that only look like credentials is a second table to keep right, and
of the two ways to be wrong, indexing a real key because it resembled an example
is the one that costs something. `--include-secrets` indexes such a file anyway,
and the run then says how many files the scan would have refused — a store built
with that flag should be able to tell you what it took in.

**A file that today's rules refuse leaves the store on the next pass.** Both
halves of the decision evict: a path refused by the deny-list and a file refused
by the content scan have their rows and vectors removed in the same run, and the
refusal line says so. That is how a `dev.env` an older release indexed leaves,
and how a file that was clean when it was indexed and has since gained a token
leaves. The file on disk is untouched.

**For a store indexed before all of this, there is `semlith scan`.** It runs
both halves over every file a store already holds — the deny-list against the
name, the content table against the text the store is holding — and prints each
file semlith would refuse today with the rule, or the kind and the line. It
exits non-zero while anything is found, so it can be a check in a script;
`--forget` evicts what it found. The portal's Privacy page has the same thing
with a button per row, reading the same function, because two implementations of
"what should not be here" would eventually disagree and the one that mattered
would be whichever you did not run. There is no MCP tool for it: an agent is not
the party that decides what a store may hold.

**The scan is a net, not a guarantee.** It refuses the shapes in that table and
nothing else. A credential with no issuer prefix, one your own service mints in
a format nobody else uses, one split across two lines, one that is base64 of
something else — none of those match, and none of them will. It lowers the odds
that an ordinary accident puts a key into a store; it is not a reason to index a
directory you would not otherwise index, and it is not a reason to skip a
`semlith scan` afterwards.

## What is yours alone on disk

`~/.semlith`, every store directory, the model cache and `~/.semlith/bin` are
created `0700` and narrowed to `0700` on every open if something loosened them.
`store.db`, the vector shards, `registry.json`, `agent.key` and `daemon.json`
are `0600`. Nothing is ever widened: a directory you set to `0500` stays `0500`.
`semlith stats` and the Stores page report a store other users can read.

```sh
stat -f '%Sp %N' ~/.semlith ~/.semlith/agent.key   # macOS
stat -c '%A %n' ~/.semlith ~/.semlith/agent.key    # Linux
```

## What is still true, and what is not covered

**The store is not encrypted.** It holds the plain text of everything you
indexed. The modes above are the boundary semlith draws; encryption at rest is a
different decision and semlith has not made it.

**The parsers are in this process.** Limits and guards are in place — an image
past 64 million pixels is refused before a decoder allocates for it, a
tree-sitter parse gives up after two seconds, a file is read once through a
capped reader — but a document you index is parsed by code running as you.
Sandboxing the parsers in a separate process is not something 0.14.0 does.

**Downgrading gives back what was refused.** A 0.13.0 binary opens every store
0.14.0 has written, ignores the trusted list and reads a `0700` directory as its
owner without complaint. What it restores is the cookie session and the
unbounded index tool — which is the honest shape of this release: the
restrictions are the change.

## Where each of these came from

Every rule on this page but one closes a finding from the security audit of
0.13.0, and each one shipped with the test that would have caught it. The
exception is the content scan, which is 0.19.0 and closes nothing: it exists
because widening what the walker indexes made "the name decides" too thin a rule
to be the only one. It shipped with its own test all the same —
`tests/scan.rs`, which walks the shape table and fails on a pattern that has
stopped matching its own example. `CHANGELOG.md` lists
them by finding id; [`docs/compatibility.md`](compatibility.md) records the four
that break something 0.13.0 did, with the way back for each.
