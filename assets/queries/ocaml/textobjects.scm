;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter-textobjects/blob/master/queries/ocaml/textobjects.scm

(value_definition
  (let_binding
    body: (_) @function.inner)) @function.outer

(method_definition
  body: (_) @function.inner) @function.outer

(class_definition
  (class_binding
    body: (_) @class.inner)) @class.outer

(for_expression
  (do_clause
    (_) @loop.inner)) @loop.outer

(while_expression
  (do_clause
    (_) @loop.inner)) @loop.outer

(if_expression
  condition: (_)
  (then_clause
    (_) @conditional.inner)
  (else_clause
    (_) @conditional.inner)) @conditional.outer

(if_expression
  condition: (_)
  (then_clause
    (_) @conditional.inner)) @conditional.outer

(function_expression
  (match_case) @_start @_end
  (match_case)* @_end
  (#make-range! "conditional.inner" @_start @_end)) @conditional.outer

(match_expression
  (match_case) @_start @_end
  (match_case)* @_end
  (#make-range! "conditional.inner" @_start @_end)) @conditional.outer

(comment) @comment.outer

(parameter) @parameter.inner
(parameter) @parameter.outer

; application arguments: `ia`/`aa` on `f a b` selects each applied arg.
; (Upstream only tags these `.outer`; we add `.inner` so `dia`/`cia` work.)
(application_expression
  argument: (_) @parameter.inner)

(application_expression
  argument: (_) @parameter.outer) @call.outer

(application_expression
  argument: (_) @_start @_end
  argument: (_)* @_end
  (#make-range! "call.inner" @_start @_end))

(parenthesized_expression
  (_) @_start @_end
  (_)? @_end
  (#make-range! "block.inner" @_start @_end)) @block.outer
