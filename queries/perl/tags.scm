(subroutine_declaration_statement name: (bareword) @name) @definition.function
(package_statement (package) @name) @definition.package
; `use constant MAX_HOLDERS => 4;`. Perl has no constant syntax at all — the
; pragma is an ordinary module doing it — so the module's name is the only
; thing that says this `use` declares one.
((use_statement
   module: (package) @_pragma
   (list_expression (autoquoted_bareword) @name)) @definition.constant
 (#eq? @_pragma "constant"))
(function_call_expression function: (function) @reference.call)
(use_statement module: (package) @reference.import)
