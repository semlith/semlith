# Security Policy

## Supported versions

semlith is pre-1.0. Only the latest release receives security fixes.

| Version | Supported |
|---|---|
| 0.1.x | ✅ |
| < 0.1 | ❌ |

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report privately through GitHub's
[private vulnerability reporting](https://github.com/semlith/semlith/security/advisories/new),
which is the preferred route. If that is unavailable to you, email
**aakashpawar1999@gmail.com** with `[semlith security]` in the subject.

Please include:

- What the issue is and roughly how bad you think it is
- Steps to reproduce, ideally with a minimal input file or command
- The version (`semlith --version`) and your OS

What to expect:

- **Acknowledgement within 72 hours.**
- An assessment and a rough timeline within 7 days.
- Credit in the release notes and the advisory, unless you would rather not be
  named.

This is a small project maintained in spare time. Fixes are made as quickly as
is practical, and we will keep you informed either way.

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

## Out of scope

- The store being readable by other users on the same machine. Set directory
  permissions appropriately; semlith does not attempt to protect against a
  local attacker who can already read your files.
- Vulnerabilities in dependencies that do not affect semlith's use of them.
  Report those upstream, though a heads-up here is welcome.
- Retrieval returning an irrelevant or unexpected chunk. That is a quality
  issue — please file it as a normal bug.
