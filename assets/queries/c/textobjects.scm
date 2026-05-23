(function_definition
  body: (compound_statement) @function.inner) @function.outer

(struct_specifier
  body: (field_declaration_list) @class.inner) @class.outer

(union_specifier
  body: (field_declaration_list) @class.inner) @class.outer

(enum_specifier
  body: (enumerator_list) @class.inner) @class.outer

(parameter_declaration) @parameter.inner
(parameter_declaration) @parameter.outer
