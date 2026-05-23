(method
  body: (_)? @function.inner) @function.outer

(singleton_method
  body: (_)? @function.inner) @function.outer

(class
  body: (_)? @class.inner) @class.outer

(singleton_class
  body: (_)? @class.inner) @class.outer

(module
  body: (_)? @class.inner) @class.outer

(method_parameters
  (_) @parameter.inner)
(method_parameters
  (_) @parameter.outer)

(block_parameters
  (_) @parameter.inner)
(block_parameters
  (_) @parameter.outer)

(lambda_parameters
  (_) @parameter.inner)
(lambda_parameters
  (_) @parameter.outer)
