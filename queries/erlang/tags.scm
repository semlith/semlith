(module_attribute name: (atom) @name) @definition.module
(fun_decl clause: (function_clause name: (atom) @name)) @definition.function
; `-define(MAX_HOLDERS, 4).` — Erlang has no constants, only preprocessor
; macros, and this is what every module writes in their place.
(pp_define lhs: (macro_lhs name: (_) @name)) @definition.constant
(call expr: (atom) @name) @reference.call
