;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/nix/folds.scm
;;
;; Vendored verbatim for vorto.

; Nix doesn't really have blocks, so just guess what people might want folds for
[
  (if_expression)
  (with_expression)
  (let_expression)
  (function_expression)
  (attrset_expression)
  (rec_attrset_expression)
  (list_expression)
  (indented_string_expression)
] @fold
