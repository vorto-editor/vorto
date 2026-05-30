(method_declaration
  body: (block) @function.inner) @function.outer

(constructor_declaration
  body: (constructor_body) @function.inner) @function.outer

(lambda_expression
  body: (_) @function.inner) @function.outer

(class_declaration
  body: (class_body) @class.inner) @class.outer

(interface_declaration
  body: (interface_body) @class.inner) @class.outer

(enum_declaration
  body: (enum_body) @class.inner) @class.outer

(record_declaration
  body: (class_body) @class.inner) @class.outer

(formal_parameter) @parameter.inner
(formal_parameter) @parameter.outer
(spread_parameter) @parameter.inner
(spread_parameter) @parameter.outer

; call arguments — `ia`/`aa` inside function calls, not just defs
(argument_list
  (_) @parameter.inner)
(argument_list
  (_) @parameter.outer)
