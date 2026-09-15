(function_declaration name: (identifier) @name) @definition.function
(class_declaration name: (identifier) @name) @definition.class
(object_declaration name: (identifier) @name) @definition.object
(property_declaration (variable_declaration (identifier) @name)) @definition.property
(call_expression (identifier) @reference.call)
(import (qualified_identifier) @reference.import)
