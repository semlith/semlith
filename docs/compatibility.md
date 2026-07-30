# Compatibility

What you can build against and expect to keep working, and what is free to
change under you. If you are wiring semlith into a script, an agent, or another
crate, this is the page that says which parts are a contract.

## What is covered

These surfaces are the public contract. A change that breaks one of them is a
break, and is treated as one.

| Surface | What is promised |
|---|---|
| CLI commands | The names `index`, `watch`, `search`, `stats`, `files`, `forget`, `mcp`, `models`, `languages`, and what each one does. |
| CLI flags | Flag names, their short forms, and their meanings — including the repeatable `--store`/`-s` on the read commands and the single `--store` the write commands take. |
| Environment | `SEMLITH_STORE` (a path-separator-delimited list, split the way `PATH` is), `SEMLITH_EMBED_THREADS`, `SEMLITH_MCP_INDEX_BUDGET`. |
| Exit codes | Whether a given outcome exits zero or non-zero. A blocked index run exits non-zero; a search that finds nothing exits zero, because finding nothing is an answer. |
| MCP tool names | `semlith_search`, `semlith_stats`, `semlith_files`, `semlith_index`, `semlith_forget`. |
| MCP input schemas | The arguments each tool accepts and their types. An existing argument does not change meaning or become required. |
| MCP protocol revisions | The list the server advertises: `2026-07-28`, `2025-11-25`, `2025-06-18`, `2024-11-05`. Dropping one is a break. |
| Store layout | A store directory holds `store.db` and `index.tv`, and a store written by one 0.x binary is readable by another (see below). |
| `src/lib.rs` | The `Semlith` type, `Hit`, `IndexReport`, and the modules `chunk`, `embed`, `filter`, `fleet`, `lock`, `mcp`, `store`, `watch`. |

## What is not covered

Everything below is deliberately outside the contract. Each is excluded for a
reason, and the reason is usually that freezing it would freeze something semlith
should be free to improve.

**Ranking scores and result ordering.** The `score` on a hit is a reciprocal
rank fusion score. It orders results within one query and means nothing across
queries or across versions. Any improvement to ranking — a better model, a
different fusion depth, a change to chunking — moves both the numbers and the
order, and that is the point of making the improvement. Do not assert on a
score, and do not assume a result stays at position three.

**Human-readable stdout and its formatting.** The text `semlith search` prints,
the columns `stats` lines up, the wording of a summary line. This is written for
a person reading a terminal, and it gets rewritten when a person reads it badly.
Parse `--json` instead; that is what it exists for.

**stderr diagnostics.** Progress, warnings, the lines the MCP server writes about
which revision it negotiated and which stores it opened. These are for
diagnosing, and their wording changes whenever a better explanation is found.
Nothing should ever be keyed off stderr text.

**The default embedding model.** It changed in 0.2.0 and it may change again when
a better one exists at the same size. This costs an existing store nothing: a
store records the model it was built with and keeps it, so a new default only
applies to a store created after the change. If you need a specific model,
name it with `--model` when the store is created.

**Additive fields.** New keys may appear in `--json` output and new lines may
appear in the text an MCP tool returns. Existing keys keep their names, types and
meanings; new ones show up beside them. Read JSON by key rather than by shape,
and ignore what you do not recognise.

**Internal SQLite schema.** That there is a `store.db` is covered. Its tables,
their columns, the FTS5 configuration and the index layout are not. They are
implementation, they have already changed within 0.x, and reading them directly
is reading past the API. Use the `store` module or the CLI.

## The honest version of the promise

semlith is 0.x. Under SemVer, a 0.x minor bump is permitted to break anything,
and this page is not going to pretend otherwise — a stability claim the version
number does not back is worth less than no claim.

What is true is the project's practice. The surfaces listed under *What is
covered* are treated as stable across 0.x releases: the intent is that a script,
an agent configuration, or a crate written against 0.5.0 keeps working on 0.6.0
and after. That is a commitment about how the project behaves, not a guarantee
the version number carries.

The one deliberate exception, already exercised: the `src/lib.rs` API took
breaking changes in 0.2.0, and it is the surface most likely to move again. The
crate is published and therefore importable, so it is documented here — but a
library consumer should pin an exact version and read the CHANGELOG before
upgrading. The CLI, the MCP tools and the store format are the surfaces the
practice most strongly covers.

This page says nothing about when 1.0 happens, because nothing has been decided.

## Store format

The store's `meta` table carries a `format_version` key, written by 0.6.0 and
later. A store without the key is format 1 — every store written before 0.6.0,
read as-is, with no migration and nothing rewritten.

A binary that opens a store whose `format_version` is higher than the format it
knows refuses it, naming both numbers, rather than reading it as best it can.
Misreading a newer store is the failure worth preventing: it does not look like
an error, it looks like a corpus that has stopped containing things.

As of 0.6.0, every 0.x store is readable by every other 0.x binary in both
directions. 0.5.0 searches a store written by 0.6.0, and 0.6.0 searches a store
written by 0.5.0 without re-embedding anything — `format_version` is an additive
meta key, which is exactly why it was safe to add before the format needed it.

## What a break would look like

If one of the covered surfaces has to change, this is what happens:

1. **It goes in the CHANGELOG**, in that version's section, stated as a break
   rather than folded into a list of improvements.
2. **The entry says why.** A rename with no reason is a rename that should not
   have happened; if the reason does not survive being written down, the change
   does not either.
3. **The entry says what to do.** The new spelling, the flag that replaces the
   old one, or the one-line edit to a configuration stanza. For a store format
   change it says whether a re-index is needed and what it costs.

Before upgrading, read the CHANGELOG section for the version you are moving to.
It is short, and a break is called out rather than buried.
