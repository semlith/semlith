(function_definition name: (identifier) @name) @definition.function
(class_declaration name: (identifier) @name) @definition.class
(interface_declaration name: (identifier) @name) @definition.interface
(method_declaration name: (identifier) @name) @definition.method
; `final MAX_HOLDERS = 4`. Groovy spells a constant with `final`, and the same
; node without it is an ordinary variable, so the modifiers are read.
((local_variable_declaration
   (modifiers) @_final
   declarator: (variable_declarator name: (identifier) @name)) @definition.constant
 (#match? @_final "\\bfinal\\b"))
(method_invocation name: (identifier) @name) @reference.call
(import_declaration (scoped_identifier) @reference.import)
