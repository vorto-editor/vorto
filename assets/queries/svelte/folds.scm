;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/svelte/folds.scm
;;
;; Adapted (not verbatim): vorto's svelte grammar is Himujjal/tree-sitter-svelte,
;; which has no `else_if_block` / `else_block` / `then_block` / `catch_block`
;; nodes (those upstream rules target a different svelte grammar). Dropped them;
;; the remaining block statements fold their else/then/catch branches with the
;; parent. `; inherits: html` is resolved against the bundled `html` query set.

; inherits: html

[
  (if_statement)
  (each_statement)
  (await_statement)
  (key_statement)
  (snippet_statement)
] @fold
