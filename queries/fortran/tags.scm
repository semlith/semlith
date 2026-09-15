(module (module_statement (name) @name)) @definition.module
(function (function_statement name: (name) @name)) @definition.function
(subroutine (subroutine_statement name: (name) @name)) @definition.function
(call_expression (identifier) @reference.call)
(use_statement (module_name) @reference.import)
