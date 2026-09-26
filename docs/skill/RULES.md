## semlith

When a semlith store covers the file you are about to open, ask semlith before you
read it and before you grep for it. The store already holds the chunks, the symbols
and the call edges, so one call returns the few hundred tokens that bear on the
question, where a whole-file read or a repository-wide grep returns thousands that do not.

`semlith_brief` is the default call: "how does X work" in one round trip, under a
token budget. `semlith_search` locates (`exact: true` lists every line matching a
string or regex, in place of grep), `semlith_read` reads one span or symbol, and
`semlith_symbol` and `semlith_neighbors` answer graph questions. Read a file
directly only to edit it, or when `semlith_files` says it is not indexed.
