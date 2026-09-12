# Security Policy

## Supported versions

semlith is pre-1.0. Only the latest release receives security fixes.

| Version | Supported |
|---|---|
| 0.12.x | ✅ |
| < 0.12 | ❌ |

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
The only network access it ever makes is downloading the ONNX embedding model
and the ONNX Runtime binaries, once, from Hugging Face and the ONNX Runtime
release page. After that it runs fully offline.

**The store is not encrypted.** `store.db` contains the plain text of every
chunk you indexed, and `index.tv` contains vectors derived from it. Anyone who
can read the store directory can read your indexed content — treat the store
with the same care as the files that went into it. If you index secrets, the
store holds secrets.

**semlith indexes whatever you point it at.** It honours `.gitignore` and skips
hidden files, but that is a convenience, not a security boundary. Check what
`semlith files` lists if you are unsure.

**`semlith start` listens on a port, and only on this machine.** Since 0.9.0 the
daemon serves a portal over HTTP. It binds `127.0.0.1` and there is no flag to
change that. Every request needs the per-run token the daemon prints once in its
URL, carried in a `SameSite=Strict; HttpOnly` cookie; without it the answer is
401 and an empty body. The `Host` header must be `localhost`, `127.0.0.1` or
`::1`, so a page on another origin cannot reach the daemon through a name that
resolves to loopback — every other `Host` gets 400 before any route runs. Every
response carries a `Content-Security-Policy` allowing only `'self'`, and no CORS
header is sent anywhere. Every byte the page loads is compiled into the binary,
so it fetches nothing. `--airgap`, or `SEMLITH_AIRGAP=1`, refuses even the model
download and exits naming the cache path.

The token grants full access to every store the daemon opened. It lives in the
URL the daemon prints and in `daemon.json` inside each store directory, which is
written `0600` on Unix. Anyone who can read that file can read the token, and
therefore the stores — the same trust boundary as the store itself. The Privacy
page has a Rotate button that invalidates the current token immediately.

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
  writer, the cookie reader and the `Host` check are all semlith's code, and a
  request that gets past the token or the `Host` check, or that wedges a worker
  thread, is exactly the kind of thing worth reporting. Header and body sizes
  are capped and sockets carry read and write timeouts, but this is new code.
- **The directory-listing route.** The portal's folder picker can list
  directories under `$HOME`. It canonicalises before checking containment, so
  `..` and symlinks are resolved first — a path that escapes that check is a
  bug worth reporting.

## Out of scope

- The store being readable by other users on the same machine. Set directory
  permissions appropriately; semlith does not attempt to protect against a
  local attacker who can already read your files. The daemon's token is in that
  same category: it sits in `daemon.json` inside the store directory, and a
  local attacker who can read the store does not need the token anyway.
- Another process on this machine connecting to the daemon with a token it
  obtained legitimately. The token guards against a web page and a stray
  process, not against a user who can already read your home directory.
- Vulnerabilities in dependencies that do not affect semlith's use of them.
  Report those upstream, though a heads-up here is welcome.
- Retrieval returning an irrelevant or unexpected chunk. That is a quality
  issue — please file it as a normal bug.
