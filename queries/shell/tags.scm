(function_definition name: (word) @name) @definition.function
; `readonly MAX_HOLDERS=4`. A bare assignment in a shell script is a variable
; that the next line can overwrite; `readonly` is the only thing that makes
; one a constant, and it shares its node with `local`, `export` and `declare`.
(declaration_command
  "readonly"
  (variable_assignment name: (variable_name) @name)) @definition.constant
(command name: (command_name (word) @name)) @reference.call
; `. ./config.sh` and `source ./config.sh` are a script pulling in another.
((command
   name: (command_name (word) @_source)
   argument: (word) @reference.import)
 (#any-of? @_source "." "source"))
