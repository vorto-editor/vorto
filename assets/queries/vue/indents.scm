; Multi-line elements (template / script / style / generic HTML
; elements) span from the opening tag's row to the closing tag's row,
; so the indent-guide engine treats them as scopes and the auto-indent
; engine fires on the opener row. Same shape as `html/indents.scm`.

[
  (element)
  (script_element)
  (style_element)
  (template_element)
] @indent.begin

(comment) @indent.ignore
