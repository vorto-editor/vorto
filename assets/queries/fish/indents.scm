;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/fish/indents.scm

[
  (function_definition)
  (while_statement)
  (for_statement)
  (if_statement)
  (begin_statement)
  (switch_statement)
] @indent.begin

[
  "else" ; else and else if must both start the line with "else", so tag the string directly
  "case"
  "end"
] @indent.branch

"end" @indent.end

(comment) @indent.ignore
