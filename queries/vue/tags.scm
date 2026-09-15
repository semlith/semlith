; The template is what this grammar parses; the script block is raw text, so a
; component's methods are not symbols here.
((element
   (start_tag
     (attribute
       (attribute_name) @_attribute
       (quoted_attribute_value (attribute_value) @name))))
 @definition.element
 (#eq? @_attribute "id"))
