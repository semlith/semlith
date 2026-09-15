; A Svelte component's own markup is what this grammar parses; the script block
; arrives as raw text, so its functions are not symbols here and nothing
; pretends otherwise.
((element
   (start_tag
     (attribute
       (attribute_name) @_attribute
       (quoted_attribute_value (attribute_value) @name))))
 @definition.element
 (#eq? @_attribute "id"))
