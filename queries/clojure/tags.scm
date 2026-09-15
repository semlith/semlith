; Clojure has no keywords, only lists whose first symbol happens to mean
; something, so a definition is recognised by shape: a list of a symbol, a
; name and an argument vector is a `defn`, and `ns` has no vector.
(list_lit
  value: (sym_lit)
  value: (sym_lit name: (sym_name) @name)
  value: (vec_lit)) @definition.function

; A list holding exactly one symbol is that symbol being called.
((list_lit . (sym_lit name: (sym_name) @name) .) @reference.call)
