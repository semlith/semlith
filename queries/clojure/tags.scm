; Clojure has no keywords, only lists whose first symbol happens to mean
; something, so a definition is recognised by shape: a list of a symbol, a
; name and an argument vector is a `defn`, and `ns` has no vector.
(list_lit
  value: (sym_lit)
  value: (sym_lit name: (sym_name) @name)
  value: (vec_lit)) @definition.function

; `(def max-holders 4)` is how Clojure names a value. The only thing telling
; it from the `defn` above is the missing argument vector, so the symbol that
; opens the list has to be read literally rather than matched by shape.
((list_lit
   value: (sym_lit name: (sym_name) @_def)
   value: (sym_lit name: (sym_name) @name)) @definition.constant
 (#eq? @_def "def"))

; A list holding exactly one symbol is that symbol being called.
((list_lit . (sym_lit name: (sym_name) @name) .) @reference.call)
