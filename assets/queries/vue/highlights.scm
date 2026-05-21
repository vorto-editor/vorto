; Minimal highlights for tree-sitter-vue (tree-sitter-grammars fork).
; Embedded `<script>` / `<style>` blocks render as plain text until
; language injection is wired up; this query covers the template layer.

(comment) @comment

(tag_name) @tag
(attribute_name) @attribute

[
  (attribute_value)
  (quoted_attribute_value)
] @string

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

"=" @operator

; Mustache-style `{{ ... }}` text interpolation
(interpolation) @punctuation.special
(interpolation
  (raw_text) @variable)

; Vue directives: `v-if`, `:prop`, `@click`, `#slot`, `.modifier`
(directive_name) @attribute
(directive_value) @variable
(directive_modifier) @function
(dynamic_directive_inner_value) @variable
