;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter-textobjects/blob/5ca4aaa6efdcc59be46b95a3e876300cfead05ef/queries/nix/textobjects.scm

; named function
(binding
  (function_expression)) @function.outer

; anonymous function
(function_expression
  (_) ; argument
  (_) @function.inner) @function.outer

(function_expression
  (formals
    (formal) @parameter.inner))

(function_expression
  (_) @parameter.outer
  (_))

(comment) @comment.outer

(if_expression
  (_) @conditional.inner) @conditional.outer

[
  (integer_expression)
  (float_expression)
] @number.inner
