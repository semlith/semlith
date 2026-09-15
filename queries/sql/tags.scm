(create_table (object_reference name: (identifier) @name)) @definition.table
(create_view (object_reference name: (identifier) @name)) @definition.view
(create_index (object_reference name: (identifier) @name)) @definition.index
; What a statement reads is what it depends on.
(from (relation (object_reference name: (identifier) @reference.table)))
