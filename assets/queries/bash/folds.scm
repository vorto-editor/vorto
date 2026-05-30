;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/bash/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (function_definition)
  (if_statement)
  (case_statement)
  (for_statement)
  (while_statement)
  (c_style_for_statement)
  (heredoc_redirect)
] @fold
