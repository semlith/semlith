; A target is the unit a Makefile is made of, and a prerequisite is one target
; depending on another — the only dependency edge a Makefile has.
(rule (targets (word) @name)) @definition.target
(rule normal: (prerequisites (word) @reference.prerequisite))
(include_directive filenames: (list (word) @reference.import))
