;; Based on nvim-treesitter (Apache-2.0).
;; Source: queries/ecma/folds.scm + queries/jsx/folds.scm
;;   @ https://github.com/nvim-treesitter/nvim-treesitter/tree/cf12346a3414fa1b06af75c79faebe7f76df080a
;;
;; nvim-treesitter's `javascript/folds.scm` is just `; inherits: ecma,jsx`.
;; vorto has no standalone `ecma`/`jsx` query set, so the inherited rules
;; are resolved inline here.

[
  (arguments)
  (for_in_statement)
  (for_statement)
  (while_statement)
  (arrow_function)
  (function_expression)
  (function_declaration)
  (class_declaration)
  (method_definition)
  (do_statement)
  (with_statement)
  (switch_statement)
  (switch_case)
  (switch_default)
  (import_statement)+
  (if_statement)
  (try_statement)
  (catch_clause)
  (array)
  (object)
  (generator_function)
  (generator_function_declaration)
] @fold

(jsx_element) @fold
