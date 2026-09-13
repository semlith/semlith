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
route accept a path only under the target store's registered roots or under your
home directory. `/etc`, another user's home, a directory nobody told semlith
about: refused, by name, with the rule that refused it.

**And never a credential.** Whatever the boundary, no path under `~/.ssh`,
`~/.aws`, `~/.gnupg`, `~/.kube`, `~/.config/gcloud`, `~/.azure`, `~/.docker`,
`~/Library/Keychains`, `~/.password-store` or `~/.local/share/keyrings` is
indexed, and neither is a file named `.env`, `.env.*`, `*.pem`, `*.key`,
`*.p12`, `*.pfx`, `*.jks`, `id_rsa*`, `id_ed25519*`, `*credentials*`,
`*secret*`, `*.tfstate` or `*.kdbx`. The hidden-file rule the walker applies now
applies to a path named explicitly too — which is how `~/.ssh/id_rsa` used to
walk past every rule the walk applied.

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

Every rule on this page closes a finding from the security audit of 0.13.0, and
each one shipped with the test that would have caught it. `CHANGELOG.md` lists
them by finding id; [`docs/compatibility.md`](compatibility.md) records the four
that break something 0.13.0 did, with the way back for each.
