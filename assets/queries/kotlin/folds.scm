;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/kotlin/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (import_list)
  (when_expression)
  (control_structure_body)
  (lambda_literal)
  (function_body)
  (primary_constructor)
  (secondary_constructor)
  (anonymous_initializer)
  (class_body)
  (enum_class_body)
  (interpolated_expression)
] @fold
