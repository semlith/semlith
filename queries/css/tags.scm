; A rule is named by the selector that introduces it, which is how a stylesheet
; is searched: nobody looks for "the third rule", they look for `.acquire`.
(rule_set (selectors (class_selector (class_name (identifier) @name)))) @definition.selector
(rule_set (selectors (id_selector (id_name) @name))) @definition.selector

; `@import url("base.css")` is this file depending on that one.
(import_statement (call_expression (arguments (string_value (string_content) @reference.import))))
