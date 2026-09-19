(class_implementation (identifier) @name) @definition.class
(class_interface (identifier) @name) @definition.interface
(method_definition (identifier) @name) @definition.method
; `#define MAX_HOLDERS 4`, the same named constant C has and for the same
; reason: the preprocessor is where a name standing for a value lives.
(preproc_def name: (identifier) @name) @definition.constant
(message_expression method: (identifier) @name) @reference.call
(preproc_include path: (_) @reference.import)
(call_expression function: (identifier) @reference.call)
