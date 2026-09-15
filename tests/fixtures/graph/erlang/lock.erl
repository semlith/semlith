-module(lock).
-export([acquire/0]).

helper() -> 1.

acquire() -> helper().
