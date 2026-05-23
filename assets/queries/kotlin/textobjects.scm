(function_declaration
  (function_body) @function.inner) @function.outer

(anonymous_function
  (function_body) @function.inner) @function.outer

(lambda_literal) @function.outer

(class_declaration
  (class_body) @class.inner) @class.outer

(object_declaration
  (class_body) @class.inner) @class.outer

(primary_constructor) @function.outer
(secondary_constructor) @function.outer

(parameter) @parameter.inner
(parameter) @parameter.outer

(class_parameter) @parameter.inner
(class_parameter) @parameter.outer

(parameter_with_optional_type) @parameter.inner
(parameter_with_optional_type) @parameter.outer
