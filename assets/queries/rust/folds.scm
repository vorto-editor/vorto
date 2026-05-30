;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/rust/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (mod_item)
  (foreign_mod_item)
  (function_item)
  (struct_item)
  (trait_item)
  (enum_item)
  (impl_item)
  (type_item)
  (union_item)
  (const_item)
  (let_declaration)
  (loop_expression)
  (for_expression)
  (while_expression)
  (if_expression)
  (match_expression)
  (call_expression)
  (array_expression)
  (macro_definition)
  (macro_invocation)
  (attribute_item)
  (block)
  (use_declaration)+
] @fold
