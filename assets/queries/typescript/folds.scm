;; Based on nvim-treesitter (Apache-2.0).
;; Source: queries/ecma/folds.scm + queries/typescript/folds.scm
;;   @ https://github.com/nvim-treesitter/nvim-treesitter/tree/cf12346a3414fa1b06af75c79faebe7f76df080a
;;
;; nvim-treesitter's `typescript/folds.scm` is `; inherits: ecma` plus the
;; TS-specific delta below. vorto has no standalone `ecma` query set, so the
;; inherited ecma rules are resolved inline. (No `jsx` — that lives in tsx.)

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

[
  (interface_declaration)
  (internal_module)
  (type_alias_declaration)
  (enum_declaration)
] @fold
