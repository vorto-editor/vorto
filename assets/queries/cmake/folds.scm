;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/cmake/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (if_condition)
  (foreach_loop)
  (while_loop)
  (function_def)
  (macro_def)
  (block_def)
] @fold
