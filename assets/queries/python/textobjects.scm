(function_definition
  body: (block) @function.inner) @function.outer

(lambda
  body: (_) @function.inner) @function.outer

(class_definition
  body: (block) @class.inner) @class.outer

(parameters
  (_) @parameter.inner)
(parameters
  (_) @parameter.outer)

(lambda_parameters
  (_) @parameter.inner)
(lambda_parameters
  (_) @parameter.outer)

; call arguments — `ia`/`aa` inside function calls, not just defs
(argument_list
  (_) @parameter.inner)
(argument_list
  (_) @parameter.outer)
