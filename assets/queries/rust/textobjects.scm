(function_item
  body: (_) @function.inner) @function.outer

(closure_expression
  body: (_) @function.inner) @function.outer

(struct_item
  body: (_) @class.inner) @class.outer

(enum_item
  body: (_) @class.inner) @class.outer

(union_item
  body: (_) @class.inner) @class.outer

(trait_item
  body: (_) @class.inner) @class.outer

(impl_item
  body: (_) @class.inner) @class.outer

(type_item
  type: (_) @type.inner) @type.outer

(parameters
  (parameter) @parameter.inner)
(parameters
  (parameter) @parameter.outer)
(parameters
  (self_parameter) @parameter.inner)
(parameters
  (self_parameter) @parameter.outer)

; call arguments — `ia`/`aa` inside function calls, not just defs
(arguments
  (_) @parameter.inner)
(arguments
  (_) @parameter.outer)
