(module (module_statement (name) @name)) @definition.module
(function (function_statement name: (name) @name)) @definition.function
(subroutine (subroutine_statement name: (name) @name)) @definition.function
; `integer, parameter :: max_holders = 4`. The `parameter` attribute is the
; whole difference between a Fortran constant and an ordinary declaration, so
; it is read rather than assumed from the shape.
((variable_declaration
   attribute: (type_qualifier) @_parameter
   declarator: (init_declarator left: (identifier) @name)) @definition.constant
 (#eq? @_parameter "parameter"))
(call_expression (identifier) @reference.call)
(use_statement (module_name) @reference.import)
