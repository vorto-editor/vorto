(function_declaration) @function.outer

; function body (the `{ ... }` block); the editor trims the braces so
; `dif`/`cif` act on the body's statements, not the braces.
(function_declaration
  body: (block) @function.inner)

; struct/enum: the node spans `struct { ... }` / `enum { ... }`, so the
; same node is outer and — once the editor trims the keyword and braces —
; inner, giving `dic`/`cic` the members without the `{ }`.
(struct_declaration) @class.outer
(struct_declaration) @class.inner
(enum_declaration) @class.outer
(enum_declaration) @class.inner

(parameter) @parameter.inner
(parameter) @parameter.outer

; call arguments — `ia`/`aa` inside function calls, not just defs
(arguments
  (_) @parameter.inner)
(arguments
  (_) @parameter.outer)
