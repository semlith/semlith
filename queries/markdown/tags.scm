; A heading is a document's symbol: it is what a link points at, what a table of
; contents lists, and what a reader is looking for. The span is the section
; rather than the heading line, so a subsection is contained by the section
; above it instead of being a sibling of it.
(section (atx_heading heading_content: (inline) @name)) @definition.heading
(section (setext_heading heading_content: (paragraph) @name)) @definition.heading
