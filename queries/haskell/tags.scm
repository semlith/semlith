; A Haskell binding is a definition whether or not it takes arguments, so
; `bind` covers both `helper = 1` and `helper x = x`.
(bind name: (variable) @name) @definition.function
(signature name: (variable) @name) @definition.signature
(import module: (module) @reference.import)
(match expression: (variable) @reference.call)
