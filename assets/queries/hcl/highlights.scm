; Minimal highlights for tree-sitter-hcl. Node names taken from the
; upstream `node-types.json`; the `_start`/`_end` named children are
; used for template interpolation markers since the grammar doesn't
; expose `"${"`/`"}"` as anonymous tokens at the query layer.

(comment) @comment

(bool_lit) @boolean
(null_lit) @constant.builtin
(numeric_lit) @number

[
  (template_literal)
  (string_lit)
  (heredoc_template)
] @string

(quoted_template_start) @string
(quoted_template_end) @string
(heredoc_start) @string
(heredoc_identifier) @string

(template_interpolation_start) @punctuation.special
(template_interpolation_end) @punctuation.special
(template_directive_start) @punctuation.special
(template_directive_end) @punctuation.special

(strip_marker) @punctuation.special

[
  "for"
  "in"
  "if"
  "else"
  "endif"
  "endfor"
] @keyword

(block (identifier) @keyword)
(attribute (identifier) @variable)
(function_call (identifier) @function)
(get_attr (identifier) @property)
(variable_expr (identifier) @variable)
