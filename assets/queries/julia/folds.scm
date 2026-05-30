;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/julia/folds.scm
;;
;; Vendored verbatim — predicates (#any-of?, #eq?, #has-ancestor?) and
;; indent directives (#set! indent.*) are supported by vorto's engine as-is.

[
  (module_definition)
  (struct_definition)
  (macro_definition)
  (function_definition)
  (if_statement)
  (try_statement)
  (for_statement)
  (while_statement)
  (let_statement)
  (quote_statement)
  (do_clause)
  (compound_statement)
] @fold
