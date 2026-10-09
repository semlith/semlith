# Security Policy

How to report a vulnerability in semlith, what to expect, and what counts as
one. The security model itself — what semlith refuses and how to check it — is
in [docs/security.md](docs/security.md).

## Supported versions

semlith is pre-1.0. **Only the latest release receives security fixes.**
`semlith upgrade --check` says whether you are on it.

## Reporting a vulnerability

**Do not open a public issue for security problems.**

Report privately through GitHub: this repository's **Security** tab →
**Report a vulnerability**, or
<https://github.com/semlith/semlith/security/advisories/new>. The report and
everything discussed on it stay private to you and the maintainers until an
advisory is published. The advisory, the fix, the credit and a private fork for
the patch all live in one place, which is why reports go there rather than to
an email address.

Include:

- what the issue is and roughly how bad you think it is;
- steps to reproduce or a proof of concept, ideally with a minimal input file
  or command;
- the version (`semlith --version`) and your OS.

What to expect:

- **Acknowledgement within 72 hours.**
- An assessment and a rough timeline within 7 days.
- Credit in the release notes and the advisory, unless you would rather not be
  named.

This is a small project maintained in spare time. Fixes ship as quickly as is
practical and you will be kept informed. Please allow reasonable time for a fix
before any public disclosure.

## What semlith does with your data

This decides what is and is not a vulnerability.

- **Nothing is sent anywhere at query time.** Embedding and search run locally.
  Every outbound request is one somebody asked for: model weights and
  accelerator packs (each pinned by digest in [docs/models.md](docs/models.md)),
  `semlith add <url>` (one request, public addresses only), `semlith upgrade`
  (`https://github.com` only; a release binary cannot be pointed at another
  origin), and Semlith Cloud after `semlith cloud login` (a token goes only to
  the host it was stored with). At build time only, `cargo install` and source
  builds download ONNX Runtime from `cdn.pyke.io` through the hash-pinning
  `ort-sys` crate; the prebuilt Linux archives instead ship Microsoft's ONNX
  Runtime release beside the binary, verified against the checksum GitHub
  publishes. `--airgap` or `SEMLITH_AIRGAP=1` refuses every runtime download
  not already in the model cache. The full list is in
  [docs/security.md](docs/security.md#what-semlith-downloads-and-when).
- **The store is not encrypted.** `store.db` holds the plain text of every chunk
  you indexed and the vector shards are derived from it. Anyone who can read the
  store directory can read your indexed content. If you index secrets, the store
  holds secrets.
- **The store records what was retrieved from it.** Every search and graph
  question — over stdio, over `/mcp`, from the CLI or the portal — appends a row
  to `store.db`'s `retrievals` table: query text, client name, session id, hit
  count and token figures. It never leaves the machine. The daemon says on every
  start that it is recording. `semlith start --no-ledger` stops it for a
  session, `SEMLITH_LEDGER=0` for a machine, and `DELETE FROM retrievals;`
  erases what was written.
- **You decide what is indexed; an agent is bounded.** On the command line,
  `.gitignore` and the hidden-file rule are conveniences, not a boundary, and
  `semlith files` shows what went in. Through MCP or the portal, indexing is
  confined to the store's registered roots or your home directory, and never
  reaches a credential directory or a file named like a credential.
- **The daemon listens on `127.0.0.1` only**, with no flag to change it. Every
  `/api/*` request needs the per-run token in a `Semlith-Token` header (401 and
  an empty body without it). A non-GET request also needs a JSON content type
  and, from a client that sends fetch metadata, `Sec-Fetch-Site: same-origin`;
  both are checked before the token and fail with 403 and an empty body. The
  `Host` header must be `localhost`, `127.0.0.1` or `::1` (otherwise 400). Every
  response carries a `Content-Security-Policy` allowing only `'self'`, no CORS
  header is sent, and every byte the portal loads is compiled into the binary.
  The page and its static assets need no credential because a browser attaches
  no header to a stylesheet, font or favicon; they are the same bytes in every
  copy of the binary.
- **The token grants full access to every store the daemon opened.** It is in
  the URL the daemon prints and in each store's `daemon.json` (mode `0600`, read
  back only from a trusted store you own that names a live process). Reading
  that file is the same trust boundary as reading the store. The Privacy page's
  Rotate button invalidates it immediately.
- **The MCP server exposes every store it opens.** `semlith mcp` opens every
  registered store unless `--store` names a set, and returns any chunk that
  matches. To limit an agent, point it at a store holding only what it should
  see.

## Known risk areas

- **PDF parsing.** `pdf-extract` runs over whatever bytes are in the file.
  Panics are caught so one malformed PDF cannot abort a run, but a crafted PDF
  causing excessive memory or CPU use is plausible and worth reporting.
- **Untrusted corpora.** Indexing a directory you do not control runs parsers
  over attacker-influenced bytes.
- **Memory during indexing.** Embedding batches are bounded; an input that
  defeats those bounds and drives the process into swap is a bug.
- **The hand-written HTTP server.** `src/http.rs` parses requests itself:
  request line, headers, the chunked response writer and the `Host` check. A
  request that gets past the token, the same-origin rule or the `Host` check, or
  that wedges a worker thread, is worth reporting. Header and body sizes are
  capped, a request has ten seconds to arrive in full, connections are capped
  at 32, and a panicking route answers 500 without losing its worker.
- **The directory-listing route.** The portal's folder picker lists directories
  under `$HOME`. It canonicalises before checking containment, so `..` and
  symlinks are resolved first; a path that escapes the check is a bug.

## Out of scope

- The store being readable by a local attacker who can already read your files.
  semlith creates what it owns `0700`/`0600` and narrows anything looser on open
  (see [docs/security.md](docs/security.md#what-is-yours-alone-on-disk)), but
  that defends against accidents and a shared machine's defaults, not against
  someone who can read your home directory. The daemon's token is in the same
  category.
- Another process on this machine using a token it obtained legitimately. The
  token guards against a web page and a stray process, not against a user who
  can read your home directory.
- Vulnerabilities in dependencies that do not affect semlith's use of them.
  Report those upstream; a heads-up here is welcome.
- Retrieval returning an irrelevant chunk. That is a quality issue; file it as a
  normal bug.
