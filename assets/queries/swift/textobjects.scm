;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter-textobjects/blob/master/queries/swift/textobjects.scm

(class_declaration
  body: (class_body
    .
    "{"
    .
    (_) @_start @_end
    (_)? @_end
    .
    "}"
    (#make-range! "class.inner" @_start @_end))) @class.outer

(class_declaration
  body: (enum_class_body
    .
    "{"
    .
    (_) @_start @_end
    (_)? @_end
    .
    "}"
    (#make-range! "class.inner" @_start @_end))) @class.outer

(function_declaration
  body: (function_body
    .
    "{"
    .
    (_) @_start @_end
    (_)? @_end
    .
    "}"
    (#make-range! "function.inner" @_start @_end))) @function.outer

(lambda_literal
  ("{"
    .
    (_) @_start @_end
    (_)? @_end
    .
    "}"
    (#make-range! "function.inner" @_start @_end))) @function.outer

(call_suffix
  (value_arguments
    .
    "("
    .
    (_) @_start
    (_)? @_end
    .
    ")"
    (#make-range! "call.inner" @_start @_end))) @call.outer

(value_argument
  value: (_) @parameter.inner) @parameter.outer

(comment) @comment.outer

(multiline_comment) @comment.outer
