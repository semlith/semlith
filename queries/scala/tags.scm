(object_definition name: (identifier) @name) @definition.object
(class_definition name: (identifier) @name) @definition.class
(trait_definition name: (identifier) @name) @definition.trait
(function_definition name: (identifier) @name) @definition.function
; `val MaxHolders = 4`. Scala's constant is a `val`, which the grammar keeps
; in a node of its own, apart from the `var` that is not one.
(val_definition pattern: (identifier) @name) @definition.constant
(call_expression function: (identifier) @reference.call)
