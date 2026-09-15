(class_implementation (identifier) @name) @definition.class
(class_interface (identifier) @name) @definition.interface
(method_definition (identifier) @name) @definition.method
(message_expression method: (identifier) @name) @reference.call
(preproc_include path: (_) @reference.import)
(call_expression function: (identifier) @reference.call)
