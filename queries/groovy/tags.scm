(function_definition name: (identifier) @name) @definition.function
(class_declaration name: (identifier) @name) @definition.class
(interface_declaration name: (identifier) @name) @definition.interface
(method_declaration name: (identifier) @name) @definition.method
(method_invocation name: (identifier) @name) @reference.call
(import_declaration (scoped_identifier) @reference.import)
