; C# indent query. `@indent.begin` opens an indent scope on the row its
; node starts; vorto uses it for indent guides and auto-indent. The
; `declaration_list` covers namespace / class / struct / interface bodies
; (the outermost guides), `block` covers method and statement bodies.

[
  (declaration_list)
  (enum_member_declaration_list)
  (block)
  (switch_body)
  (accessor_list)
  (initializer_expression)
  (anonymous_object_creation_expression)
  (argument_list)
  (parameter_list)
  (bracketed_argument_list)
  (bracketed_parameter_list)
] @indent.begin

[
  "}"
  "]"
  ")"
] @indent.end

[
  "{"
  "}"
  "("
  ")"
  "["
  "]"
] @indent.branch

[
  (comment)
  (string_literal)
  (verbatim_string_literal)
] @indent.ignore
