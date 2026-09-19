-module(lock).
-export([acquire/0]).

-define(MAX_HOLDERS, 4).

helper() -> 1.

acquire() -> helper().
