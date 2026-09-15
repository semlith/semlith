; A block's label is its name — `resource "aws_s3_bucket" "lock"` is `lock` —
; so the label nearest the body is the one captured.
(block (identifier) (string_lit (template_literal) @name) . (block_start)) @definition.block
