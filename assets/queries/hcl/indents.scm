;; HCL / Terraform indent query.
;;
;; Blocks (`name "label" { ... }`), objects (`{ ... }`) and tuples
;; (`[ ... ]`) are the indentation-bearing containers — each opens one
;; level for the lines it spans. In this grammar the opening/closing
;; delimiters are named nodes (block_start/_end, object_start/_end,
;; tuple_start/_end), not bare `{}` / `[]` tokens, so the dedent and
;; branch captures target those.

[
  (block)
  (object)
  (tuple)
] @indent.begin

[
  (block_end)
  (object_end)
  (tuple_end)
] @indent.end

[
  (block_start)
  (block_end)
  (object_start)
  (object_end)
  (tuple_start)
  (tuple_end)
] @indent.branch

(comment) @indent.ignore
