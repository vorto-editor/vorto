(function_declaration
  body: (block) @function.inner) @function.outer

(method_declaration
  body: (block) @function.inner) @function.outer

(func_literal
  body: (block) @function.inner) @function.outer

(type_declaration
  (type_spec
    type: (struct_type
      (field_declaration_list) @class.inner))) @class.outer

(type_declaration
  (type_spec
    type: (interface_type) @class.inner)) @class.outer

(type_spec
  name: (_) @_n
  type: (_) @type.inner) @type.outer

(type_alias
  name: (_) @_an
  type: (_) @type.inner) @type.outer

(parameter_declaration) @parameter.inner
(parameter_declaration) @parameter.outer
(variadic_parameter_declaration) @parameter.inner
(variadic_parameter_declaration) @parameter.outer

; call arguments — `ia`/`aa` inside function calls, not just defs
(argument_list
  (_) @parameter.inner)
(argument_list
  (_) @parameter.outer)
