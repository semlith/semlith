---
name: semlith
description: Use FIRST for any question about code in a folder semlith has indexed — where X is (semlith_search), who calls it (semlith_neighbors), what breaks if it changes (semlith_impact), every line matching a string or regex (semlith_search exact: true), what a PDF, Word, PowerPoint or Excel file says — before grep, rg, find, cat or Read. Skip only for git history and the exact read right before an edit.
---

# semlith

The `semlith_*` MCP tools hold a pre-built index of the folders semlith covers:
chunks, embeddings, symbols and call edges. One call returns the lines that bear
on the question with a `path:line` you can cite.

## Pick the tool by the question

| The question | Call |
|---|---|
| Where is X? / how does X work? / which files handle X? | `semlith_search {query}` |
| Every line matching a string or regex (every TODO) | `semlith_search {query, exact: true}` |
| Who calls X? / every call site of X | `semlith_neighbors {name}` |
| What breaks if X changes? | `semlith_impact {name}` |
| This span / this whole function | `semlith_read {target}`: `path:start-end` or a symbol |
| Where are these defined? | `semlith_symbol {names: [...]}`, up to 20 |
| A span plus its callers and callees in one call | `semlith_brief {question}` |
| What is in this directory? | `semlith_files {tree: true}` |
| What does this PDF, Word, PowerPoint, Excel or EPUB file say? | `semlith_search {query, ext: ["pdf"]}`, then `semlith_read` |

**Documents.** semlith indexes PDF, Word, PowerPoint, Excel, OpenDocument, EPUB
and HTML as text. Grep and Read see those files' bytes, not their words: answer
and quote from semlith, never from a guess about what such a file contains.

A store that holds several repositories: pass `path: ["<repo>/**"]` to every
call. Unscoped, a name defined in another repository can answer first; scoped,
search, symbol, neighbors, impact and brief see only that repository.

`path`, `ext` and `lang` take globs; a leading `!` excludes. Tests rank below
product code unless the query names tests.

## Rules

1. **First lookup is semlith**, not grep, rg, find, cat, sed or Read on indexed
   source. Use Read only for the exact text right before an edit.
2. **Delegating?** Subagents do not see this skill. Tell them: "Use the semlith
   MCP tools for every code lookup, semlith_search with exact: true in place of
   grep." In Claude Code, pick `semlith-explorer` over `Explore`.
3. **Fall back per question.** If an answer is thin, say so, use grep/Read for
   that question only, and return to semlith for the next.
4. Never change the index unless the user asked. The CLI does it:
   `semlith index <path>`, `semlith forget <file>`.

Only indexed paths are known: check `semlith_files` before concluding something
does not exist.
