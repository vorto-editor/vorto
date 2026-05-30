(function_declaration
  body: (block) @function.inner) @function.outer

(function_definition
  body: (block) @function.inner) @function.outer

(parameters
  (identifier) @parameter.inner)
(parameters
  (identifier) @parameter.outer)
(parameters
  (vararg_expression) @parameter.inner)
(parameters
  (vararg_expression) @parameter.outer)

; call arguments — `ia`/`aa` inside function calls, not just defs
(arguments
  (_) @parameter.inner)
(arguments
  (_) @parameter.outer)
