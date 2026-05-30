; Scala indent query. Hand-authored for vorto's `@indent.begin` convention
; (nvim-treesitter ships no scala `indents.scm` at the pinned commit). The
; `template_body` covers class / object / trait bodies — the outermost
; indent guides — while `block` / `indented_block` cover method and
; statement bodies, including Scala 3 significant-indentation regions.

[
  (template_body)
  (block)
  (case_block)
  (indented_block)
  (indented_cases)
  (arguments)
  (parameters)
  (type_arguments)
] @indent.begin

[
  "}"
  ")"
  "]"
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
  (string)
  (interpolated_string)
] @indent.ignore
