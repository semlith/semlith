---
name: semlith
description: Use FIRST for any question about code in a folder semlith has indexed — how something works (semlith_brief), where it is defined or used, what calls it, what breaks if it changes, how A reaches B, what a directory holds, or where a planned change would land — before grep, rg, git grep, find, cat, sed or Read. Also use when handing code research to a subagent. Skip only for literal-text sweeps (every TODO, every occurrence of a string) and the exact read right before an edit.
---

# semlith

The `semlith_*` MCP tools hold a pre-built index of the folders semlith covers:
chunks, embeddings, parsed symbols and call edges. One call replaces a round of
grep + read, returns only the lines that bear on the question, and gives
`path:line` you can cite. Make a semlith call your first lookup for every code
question, and keep using it until the question is answered.

## Pick the tool by the question

| The question | Call |
|---|---|
| How does X work? / explain X | `semlith_brief {question}`, in the user's words |
| Where is X? / which files handle X? | `semlith_search {query}` |
| What breaks if X changes? / who calls X? / every call site | `semlith_impact {name}` |
| What does X call, one hop either way? | `semlith_neighbors {name}` |
| How does A reach B? / trace the flow | `semlith_trace {from, to}` (`semlith_path` for yes or no) |
| This span / this whole function | `semlith_read {target}`: `path:start-end` or a symbol name |
| Several definitions at once | `semlith_symbol {names: [...]}`, up to 20 |
| What is in this directory? / where would X live? | `semlith_files {tree: true}` |
| Every construct of shape Y | `semlith_pattern {query, lang}`, a tree-sitter query |

Planning a change or tracing a flow: `semlith_brief` for the core, then
`semlith_impact` or `semlith_trace` on the central symbols it names, then read
each central symbol whole (`semlith_read` by name) before you answer.

## What the tools take and return

- `semlith_impact` accepts `Type::method`, `module::function` and `Type.method`.
  Rows carry call-site lines. Past 16 000 characters it gives per-file counts
  and a `more:` line. `semlith_symbol` and `semlith_neighbors` share that cap; a
  caller of a name with several definitions shows `→ Type::method`.
- `semlith_read` takes paths relative to the store root. A symbol name returns
  every definition whole (`Type::method` narrows), up to 32 000 characters. A
  file edited since indexing is read from disk and marked.
- `semlith_search` rows read `start-end name kind @defline · lists | best line`,
  with paths relative to the `root …` header. Identical copies collapse to
  `also in N copies`. `format: "excerpt"` adds text.
- `semlith_brief` gives text for one span, the best code span for a code
  question, plus one-hop edges. `semlith_read` the other spans you need.
- `semlith_files {tree: true, depth, sort: name|size|symbols|recent}` shows
  directories with counts and languages, per-file lines, symbols and first
  definitions, and what is on disk but not indexed, and why. Capped at 8 000
  characters.

`path`, `ext` and `lang` filter search, files and pattern: globs, repeats
union, kinds intersect, a leading `!` excludes. Tests and fixtures rank below
product code unless the query names tests. For a question about product code,
add `path: ["src/**"]` (or the repo's source root) to keep them out entirely.
Filtering happens before ranking, so narrowing improves the answer too.

## Rules

1. **First lookup is semlith.** Do not open with grep, rg, find, cat, sed or
   Read on indexed source. Bash `grep`/`rg` is the same habit as the Grep tool.
2. **Read spans, not files.** Use the host `Read` (with `offset`/`limit`) only
   for the exact text right before an edit, or for a file that is not indexed.
3. **Delegating?** Subagents do not see this skill. Put this line in their
   prompt: "Use the semlith MCP tools (semlith_brief, semlith_search,
   semlith_impact, semlith_read) for every code lookup; grep only for
   literal-text sweeps." In Claude Code, pick `semlith-explorer` over `Explore`.
4. **Fall back per question, not per session.** If an answer is thin or wrong,
   say so in one line, use grep/Read for that question only, and return to
   semlith for the next.
5. **Never call `semlith_index`, `semlith_add` or `semlith_forget`** unless the
   user asked to change the index. They write; a refusal is not a reason to.

## Limits

- 16 tools: `semlith_search`, `semlith_brief`, `semlith_read`, `semlith_symbol`,
  `semlith_neighbors`, `semlith_path`, `semlith_trace`, `semlith_impact`,
  `semlith_files`, `semlith_pattern`, `semlith_stats`, `semlith_languages`,
  `semlith_report`, `semlith_index`, `semlith_add`, `semlith_forget`.
- No regular expressions. For every literal occurrence of a string, grep is
  the right tool.
- Multi-store search merges on rank, so never compare scores across stores.
- Only indexed paths are known. Check `semlith_files` before concluding that
  something does not exist.
