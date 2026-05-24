;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/elixir/indents.scm

[
  (block)
  (do_block)
  (list)
  (map)
  (stab_clause)
  (tuple)
  (arguments)
] @indent.begin

[
  ")"
  "]"
  "after"
  "catch"
  "else"
  "rescue"
  "}"
  "end"
] @indent.end @indent.branch

; Elixir pipelines are not indented, but other binary operator chains are
((binary_operator
  operator: _ @_operator) @indent.begin
  (#not-eq? @_operator "|>"))
