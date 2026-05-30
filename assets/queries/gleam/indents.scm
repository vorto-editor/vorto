;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/gleam/indents.scm
;;
;; Vendored verbatim for vorto.

; Gleam indents similar to Rust and JavaScript
[
  (anonymous_function)
  (assert)
  (case)
  (case_clause)
  (constant)
  (external_function)
  (function)
  (let)
  (list)
  (constant)
  (function)
  (type_definition)
  (type_alias)
  (todo)
  (tuple)
  (unqualified_imports)
] @indent.begin

[
  ")"
  "]"
  "}"
] @indent.end @indent.branch

; Gleam pipelines are not indented, but other binary expression chains are
((binary_expression
  operator: _ @_operator) @indent.begin
  (#not-eq? @_operator "|>"))
