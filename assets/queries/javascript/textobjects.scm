(function_declaration
  body: (statement_block) @function.inner) @function.outer

(function_expression
  body: (statement_block) @function.inner) @function.outer

(generator_function_declaration
  body: (statement_block) @function.inner) @function.outer

(generator_function
  body: (statement_block) @function.inner) @function.outer

(arrow_function
  body: (_) @function.inner) @function.outer

(method_definition
  body: (statement_block) @function.inner) @function.outer

(class_declaration
  body: (class_body) @class.inner) @class.outer

(class
  body: (class_body) @class.inner) @class.outer

(formal_parameters
  (_) @parameter.inner)
(formal_parameters
  (_) @parameter.outer)
