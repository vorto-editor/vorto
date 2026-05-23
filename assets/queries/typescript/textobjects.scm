; inherits: javascript

(interface_declaration
  (interface_body) @class.inner) @class.outer

(abstract_class_declaration
  body: (class_body) @class.inner) @class.outer

(required_parameter) @parameter.inner
(required_parameter) @parameter.outer
(optional_parameter) @parameter.inner
(optional_parameter) @parameter.outer
