# Security Policy

## Supported versions

semlith is pre-1.0. **Only the latest release receives security fixes**, and
that is the whole policy — the table this section used to carry named a version
that was already two behind by the time anybody read it.

`semlith upgrade --check` says whether you are on it.

## Reporting a vulnerability

**Do not open a public issue for security problems.**

Report privately through GitHub, on this repository's **Security** tab →
**Report a vulnerability**, or directly at
<https://github.com/semlith/semlith/security/advisories/new>. The report, and
everything discussed on it, stays private to you and the maintainers until an
advisory is published.

Please include:

- What the issue is and roughly how bad you think it is
- Steps to reproduce or a proof of concept, ideally with a minimal input file
  or command
- The version (`semlith --version`) and your OS

Reporting this way rather than by email is deliberate: the advisory, the fix and
the credit live in one place, and a private fork for the patch comes with it.

What to expect:

- **Acknowledgement within 72 hours.**
- An assessment and a rough timeline within 7 days.
- Credit in the release notes and the advisory, unless you would rather not be
  named.

This is a small project maintained in spare time. Fixes are made as quickly as
is practical, and we will keep you informed either way. Please give us
reasonable time to release a fix before any public disclosure.

## What semlith actually does with your data

Worth being explicit, because it determines what is and is not a vulnerability
here.

**Nothing is sent anywhere at query time.** semlith embeds and searches locally.
It makes exactly five kinds of outbound request, all of them because somebody
asked:

- **Model weights**, once per model, from Hugging Face, at a pinned commit and
  verified against a digest recorded in [`docs/models.md`](docs/models.md).
  That includes the fp16 export the GPU lane runs.
- **Accelerator components**, pinned by digest in the same file. Microsoft's
  WebGPU plugin comes from its PyPI wheel on `files.pythonhosted.org`. It is
  fetched, with the fp16 weights, on the first index run in a daemon that has
  found a hardware GPU with the GPU lane on, and never where the only adapter
  is a software renderer. The CUDA pack is ONNX Runtime's GPU build from
  `github.com` and NVIDIA's CUDA libraries from their wheels on
  `files.pythonhosted.org`. It is fetched only after somebody turns CUDA on,
  and its size, 1.89 GB, is stated before the download starts. semlith never
  hosts or re-distributes NVIDIA's libraries.
- **`semlith add <url>`**, one request for exactly that URL. No crawling, no
  credential, and from 0.14.0 no address that is not on the public internet.
- **`semlith upgrade`**, from `https://github.com` and nowhere else — a release
  binary has no way to be told another origin.
- **At build time only**, ONNX Runtime itself. A `cargo install` or a
  from-source build downloads it from `cdn.pyke.io` through the `ort-sys` crate,
  which hash-pins what it fetches. The prebuilt Linux binaries do not use it at
  all: from 0.14.0 they ship Microsoft's own ONNX Runtime release beside them,
  verified against the checksum GitHub publishes with it.

`--airgap`, or `SEMLITH_AIRGAP=1`, refuses all of the runtime ones, apart from
files already present in the model cache, and exits naming what it refused.
After the weights and any accelerator components are cached, semlith runs fully
offline.

**The store is not encrypted.** `store.db` contains the plain text of every
chunk you indexed, and `index.tv` contains vectors derived from it. Anyone who
can read the store directory can read your indexed content — treat the store
with the same care as the files that went into it. If you index secrets, the
store holds secrets.

**From 0.15.0 the store also records what was retrieved from it.** Every search
and every graph question — from an agent over stdio, from an agent over `/mcp`,
from the command line, from the portal — appends a row to the `retrievals` table
inside `store.db`: the query text, the client's own name, a session id, the hit
count and the token figures. Recording is on by default, where through 0.14.0 it
was off unless `semlith start --ledger` asked for it.

That row is part of the store, and everything above about the store applies to
it: it is not encrypted, and anyone who can read the store directory can read
your query history. It is also the reason the default changed — a ledger nobody
switched on has no rows, and an audit trail with no rows is not one.

Nothing about it leaves this machine, and that claim is unchanged and still
checkable the same way as every other claim on this page: a packet capture. The
daemon states on every start that it is recording and names the flag that stops
it. `semlith start --no-ledger` records nothing for that session,
`SEMLITH_LEDGER=0` records nothing on that machine, and
`DELETE FROM retrievals;` erases every row that was already written. The
`--ledger` flag is gone rather than kept as a switch that does nothing, so a
script still passing it fails at parse time rather than quietly meaning its
opposite.

**semlith indexes whatever *you* point it at — an agent is bounded.** On the
command line, `.gitignore` and the hidden-file rule are a convenience rather
than a boundary, and `semlith files` is how you check what went in. Through the
MCP tools or the portal it is a boundary: from 0.14.0 those index only under the
store's registered roots or your home directory, and never a credential
directory or a file named like a credential. [`docs/security.md`](docs/security.md)
has the list and the reasoning.

**`semlith start` listens on a port, and only on this machine.** Since 0.9.0 the
daemon serves a portal over HTTP. It binds `127.0.0.1` and there is no flag to
change that.

Every `/api/*` request needs the per-run token the daemon prints once in its
URL, sent back in a **`Semlith-Token` header**. It was a `SameSite=Strict`
cookie until 0.14.0, and a cookie was the wrong container: every port on
`localhost` is the same site, so the browser would attach it for a page served
by anything else on `127.0.0.1`. Without the token the answer is 401 and an
empty body. A request that is not a GET additionally needs a JSON content type
and, from any client that sends fetch metadata, `Sec-Fetch-Site: same-origin`;
both are checked before the token, and a failure is 403 with an empty body
before any route runs.

The page itself and its own static assets are served without a credential,
because a browser attaches no header to a stylesheet, a font or a favicon. They
are the same bytes in every copy of the binary.

The `Host` header must be `localhost`, `127.0.0.1` or `::1`, so a page cannot
reach the daemon through a name that resolves to loopback — every other `Host`
gets 400 before any route runs. Every response carries a
`Content-Security-Policy` allowing only `'self'`, and no CORS header is sent
anywhere. Every byte the page loads is compiled into the binary.

The token grants full access to every store the daemon opened. It lives in the
URL the daemon prints and in `daemon.json` inside each store directory, written
`0600` and — from 0.14.0 — read back only from a store you trust, only when it
is owned by you and names a live process. Anyone who can read that file can read
the token, and therefore the stores: the same trust boundary as the store
itself. The Privacy page has a Rotate button that invalidates the current token
immediately, and a row per rule with the daemon's own check of it.

**The MCP server exposes the whole store.** `semlith mcp` speaks over stdio to
whatever process launched it and will return any indexed chunk that matches a
query. Scope this with `--store`: point an agent at a store containing only what
that agent should see.

## Known risk areas

If you are looking for somewhere to dig, these are the honest weak points:

- **PDF parsing.** `pdf-extract` runs over whatever bytes are in the file.
  Panics are caught so one malformed PDF cannot abort an indexing run, but a
  crafted PDF causing excessive memory or CPU use is plausible and worth
  reporting.
- **Untrusted corpora generally.** Indexing a directory you do not control
  means running parsers over attacker-influenced bytes.
- **Memory during indexing.** Embedding batches are bounded deliberately; an
  input that defeats those bounds and drives the process into swap is a real
  bug, and we would like to know about it.
- **The hand-written HTTP server.** `src/http.rs` parses requests itself rather
  than through a crate. Request line and header parsing, the chunked response
  writer, the header reader and the `Host` check are all semlith's code, and a
  request that gets past the token, the same-origin rule or the `Host` check, or
  that wedges a worker thread, is exactly the kind of thing worth reporting.
  Header and body sizes are capped, a request has ten seconds to arrive in full,
  connections are capped at 32 at once, and a panicking route answers 500
  without taking its worker — but this is still hand-written code on a socket.
- **The directory-listing route.** The portal's folder picker can list
  directories under `$HOME`. It canonicalises before checking containment, so
  `..` and symlinks are resolved first — a path that escapes that check is a
  bug worth reporting.

## Out of scope

- The store being readable by a local attacker who can already read your files.
  From 0.14.0 semlith creates everything it owns `0700`/`0600` and narrows
  anything looser on open — see [`docs/security.md`](docs/security.md) — but
  that is defence against an accident and a shared machine's defaults, not
  against somebody who can read your home directory. The daemon's token is in
  that same category: a local attacker who can read the store does not need it.
- Another process on this machine connecting to the daemon with a token it
  obtained legitimately. The token guards against a web page and a stray
  process, not against a user who can already read your home directory.
- Vulnerabilities in dependencies that do not affect semlith's use of them.
  Report those upstream, though a heads-up here is welcome.
- Retrieval returning an irrelevant or unexpected chunk. That is a quality
  issue — please file it as a normal bug.
