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
