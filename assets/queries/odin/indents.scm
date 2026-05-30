;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/odin/indents.scm
;;
;; Vendored verbatim for vorto.

[
  (block)
  (enum_declaration)
  (union_declaration)
  (bit_field_declaration)
  (struct_declaration)
  (struct)
  (parameters)
  (tuple_type)
  (call_expression)
  (switch_case)
] @indent.begin

; hello(
((identifier)
  .
  (ERROR
    "(" @indent.begin))

[
  ")"
  "]"
] @indent.branch @indent.end

; Have to do all closing brackets separately because the one for switch statements shouldn't end.
(block
  "}" @indent.branch @indent.end)

(enum_declaration
  "}" @indent.branch @indent.end)

(union_declaration
  "}" @indent.branch @indent.end)

(struct_declaration
  "}" @indent.branch @indent.end)

(struct
  "}" @indent.branch @indent.end)

[
  (comment)
  (block_comment)
  (string)
  (ERROR)
] @indent.auto
