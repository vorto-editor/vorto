(function_definition
  body: (compound_statement) @function.inner) @function.outer

(lambda_expression
  body: (compound_statement) @function.inner) @function.outer

(class_specifier
  body: (field_declaration_list) @class.inner) @class.outer

(struct_specifier
  body: (field_declaration_list) @class.inner) @class.outer

(union_specifier
  body: (field_declaration_list) @class.inner) @class.outer

(enum_specifier
  body: (enumerator_list) @class.inner) @class.outer

(parameter_declaration) @parameter.inner
(parameter_declaration) @parameter.outer

(optional_parameter_declaration) @parameter.inner
(optional_parameter_declaration) @parameter.outer

(variadic_parameter_declaration) @parameter.inner
(variadic_parameter_declaration) @parameter.outer
