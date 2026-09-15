; An element with an id is the thing anything else on the page refers to, so
; that is what a page's symbols are. The predicate has to sit inside the
; pattern's own parentheses; outside them it is a pattern of its own and
; filters nothing, which is how `rel="stylesheet"` briefly became a symbol.
((element
   (start_tag
     (attribute
       (attribute_name) @_attribute
       (quoted_attribute_value (attribute_value) @name))))
 @definition.element
 (#eq? @_attribute "id"))

; What the page pulls in: a stylesheet, a script, an image, another page.
((element
   (start_tag
     (attribute
       (attribute_name) @_link
       (quoted_attribute_value (attribute_value) @reference.import))))
 (#any-of? @_link "href" "src"))

((script_element
   (start_tag
     (attribute
       (attribute_name) @_src
       (quoted_attribute_value (attribute_value) @reference.import))))
 (#eq? @_src "src"))
