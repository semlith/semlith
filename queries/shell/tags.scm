(function_definition name: (word) @name) @definition.function
(command name: (command_name (word) @name)) @reference.call
; `. ./config.sh` and `source ./config.sh` are a script pulling in another.
((command
   name: (command_name (word) @_source)
   argument: (word) @reference.import)
 (#any-of? @_source "." "source"))
