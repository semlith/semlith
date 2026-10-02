## semlith

When a semlith store covers the file you are about to open, ask semlith before you
read it or grep for it: one call returns the lines that bear on the question.

`semlith_search` locates (`exact: true` lists every line matching a string or
regex, in place of grep), `semlith_neighbors` answers who calls a symbol,
`semlith_impact` what breaks if it changes, and `semlith_read` reads one span or
symbol. Scope a store of several repositories with `path`. Read a file directly
only to edit it, or when `semlith_files` says it is not indexed.
