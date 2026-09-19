---
name: semlith
description: Answer questions about a codebase through a local semlith store over MCP rather than by reading whole files or grepping the repository. Use when the question is where something lives, how a piece of code works, what calls a symbol or what it calls, or what shape a construct takes across a language, and the semlith MCP tools (semlith_search, semlith_brief, semlith_read, semlith_symbol, semlith_neighbors, semlith_pattern) are connected. Not for editing code, for regular expressions, or for fetching anything beyond one URL you name.
---

# semlith

semlith is a local vector and graph index over a folder tree, served over MCP.
It already holds the chunks, the parsed symbols and the call edges for every
file it has indexed, so the work of finding and reading code has been done once,
offline, rather than once per question at your context's expense. Use it as the
first move, not the fallback.

Thirteen tools are exposed: `semlith_search`, `semlith_brief`, `semlith_read`,
`semlith_pattern`, `semlith_symbol`, `semlith_neighbors`, `semlith_path`,
`semlith_stats`, `semlith_files`, `semlith_languages`, `semlith_index`,
`semlith_add` and `semlith_forget`. Six of them cover almost every question.

## Start with semlith_brief

For any question of the form "how does X work", call `semlith_brief` first, with
the question in the words the user asked it. One call returns the spans a search
would have found, the text of the top ones, and the one-hop callers and callees
of the symbols those spans sit inside, every part labelled with the list or edge
that produced it, all under a token `budget` that defaults to 4000. That is the
search, the read and the neighbour walk collapsed into a single round trip, and
it is bounded, so it cannot flood the conversation the way an open-ended read
can. When the budget is tight the answer drops span text before it drops
locators and edges, and it says what it dropped, so a thin answer is a signal to
raise `budget` rather than to start grepping.

Reach past `brief` only when the question is narrower than it:

- `semlith_search` when you need to locate rather than understand — where
  something is mentioned, which files are involved. `format: "excerpt"` adds the
  text when the locators alone are not enough; the default `locate` is cheaper
  and usually sufficient as the first half of a two-step.
- `semlith_read` for one span or one symbol once you know its name or its
  `path:start-end`, and nothing around it. This is the second stage after a
  search, and it answers out of the store's chunks rather than off disk.
- `semlith_symbol` for a definition with its callers, its callees and the ring
  two hops out, read off the parsed syntax tree rather than matched in a comment
  or a string. `history: true` says what the name used to be.
- `semlith_neighbors` for the one-hop question on its own: what calls this, what
  does it call. Every edge carries how well it is supported — trust `extracted`
  and `resolved`, read `inferred` as a hint, and treat `ambiguous` as a question
  about which of several same-named definitions was meant.
- `semlith_path` for whether two symbols are connected at all, and by what
  chain. A refusal is an answer: it means there is no route that does not cross
  a name the store cannot pin down.
- `semlith_pattern` for structure rather than meaning — every call whose callee
  is an identifier, every function with a particular parameter shape. It takes a
  tree-sitter query and a required `lang`, and it asks the kind of question a
  text search cannot express.

`semlith_stats`, `semlith_files` and `semlith_languages` are orientation: what
the open stores hold, which files are actually indexed, and the language names
`lang` accepts. Check `semlith_files` before concluding that something does not
exist, because "not indexed" and "not present" look identical from a search.

## Filters

`semlith_search`, `semlith_files` and `semlith_pattern` all narrow the same way,
with `path` globs, `ext` extensions and `lang` language names. Repeats union and
kinds intersect: `ext: ["rs", "toml"]` means Rust or TOML, while
`path: ["src/**"], ext: ["md"]` means Markdown under `src`. A leading `!`
excludes, after the inclusions of its own kind, so
`path: ["src/**", "!src/vendor/**"]` is everything under `src` but the vendored
tree. Exclusion is new in 0.24.0.

Filtering happens before either half of the search ranks anything, so asking for
eight hits under one directory returns the eight best hits in that directory,
not whatever survives filtering the repository's eight best. Narrow early; it
improves the answer as well as the cost.

## Do not re-do work the store has done

Never read a whole file the store already holds, and never run a repository-wide
grep, when one semlith call answers the question: the call returns the few
hundred tokens that bear on it, the file or the grep returns thousands that do
not. Say that once, in a line, and move on — a paragraph defending the choice
costs more than the choice saved.

Two honest exceptions. You still need a real read of a file before editing it,
because a store returns chunks and an exact edit needs exact current text. And
when `semlith_files` shows a path is not indexed, the store cannot answer for it
at all, so read it directly or index it with `semlith_index`.

## What semlith does not do

Knowing the edges saves you attempting them:

- No regular expressions. Filters are SQLite `GLOB` patterns, where `*` crosses
  `/` and matching ignores case, and a leading `!` is the only negation. For a
  structural question use `semlith_pattern`; for a literal string a text search
  is still the right tool.
- No writes to your code. `semlith_index`, `semlith_add` and `semlith_forget`
  change the index only. `semlith_forget` drops a file from a store and leaves
  the file on disk untouched.
- No crawling and no browsing. `semlith_add` fetches exactly the one https URL
  you give it, and is refused outright under `--airgap`.
- No reranking, and no reverse reachability. Multi-store search merges on rank
  rather than ranking jointly, so never compare scores across stores.
- No knowledge of anything outside the indexed paths. Image search matches what
  a picture depicts and is not OCR, and the default text model is English-only.

Every retrieval is recorded in the store's local ledger, which never leaves the
machine. That is a reason to ask semlith freely, not a reason to hesitate.
