; A build stage is the unit a Dockerfile is made of, and the image each stage
; starts `FROM` is the dependency that a base-image change propagates through.
(from_instruction as: (image_alias) @name) @definition.stage
(from_instruction (image_spec name: (image_name) @reference.import))
