;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/dart/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (class_definition)
  (enum_declaration)
  (extension_declaration)
  (arguments)
  (function_body)
  (block)
  (switch_block)
  (list_literal)
  (set_or_map_literal)
  (string_literal)
  (import_or_export)+
] @fold
