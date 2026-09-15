(function_statement (function_name) @name) @definition.function
(class_statement (simple_name) @name) @definition.class
; PowerShell has no call syntax distinct from running a command, so every
; command is a reference and the resolver decides which ones name something
; this store holds.
(command command_name: (command_name) @reference.call)
