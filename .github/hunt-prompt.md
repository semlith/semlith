You are testing `semlith`, a local vector store CLI with an MCP server and a web portal, on this machine's native operating system. The binary is already installed and on PATH; it was installed by the same installer a user runs. The repository checked out in the current directory is its source, and `README.md` documents every command.

Your job is to find bugs a real user on this operating system would hit. Do not build from source and do not fix anything. Exercise the installed binary the way a new user would:

1. Read `README.md` sections "Quick start", "Commands", "Where stores live", "Using it from an agent" and "Keeping the store current".
2. Run every command in the Commands table with realistic arguments, including the error paths: missing arguments, a path that does not exist, a store that is not there, a port already in use, a query with quotes and unicode, paths with spaces, and re-running commands that should be idempotent.
3. Start `semlith start` in the background and check the portal and the HTTP MCP endpoint with curl, then stop it cleanly and confirm nothing is left running and no lock is left behind.
4. Drive `semlith mcp` over stdin with JSON-RPC `initialize`, `tools/list` and a `tools/call`, and check the responses are well-formed.
5. Look for anything specific to this operating system: path separators, line endings, case-insensitive filesystems, PATH handling, permission bits, console encoding, process signals, and how the installer left `~/.semlith`.

Write the report to stdout in Markdown with this shape. Be concrete: exact command, exact output (trim long output), expected behavior, and why it matters. Rank by severity. If a command behaved correctly, do not mention it except in the final count.

# Bug hunt: <operating system and version>

## Findings
### <severity>: <one-line title>
- Command: `...`
- Output: `...`
- Expected: ...
- Why it matters: ...

## Summary
Commands exercised: N. Findings: N (N high, N medium, N low).
