(module_definition name: (identifier) @name) @definition.module
(function_definition name: (identifier) @name) @definition.function
; `helper() = 1` is a definition written as an assignment.
(assignment . (call_expression (identifier) @name (argument_list))) @definition.function
(using_statement (identifier) @reference.import)
(import_statement (identifier) @reference.import)
(assignment (operator) (call_expression (identifier) @reference.call))
