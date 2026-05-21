; Minimal highlights for tree-sitter-svelte. Embedded `<script>` /
; `<style>` blocks render as plain text until language injection is
; wired up; this query covers the template + control-flow layer.

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

; `{#if …}`, `{#each …}`, `{:else}`, `{/each}` and friends.
[
  (special_block_keyword)
  (then)
  (as)
] @keyword

[
  "{"
  "}"
  "#"
  ":"
  "/"
  "@"
] @punctuation.special

; Expression-bearing children of block tags (`each_start_expr`, etc.)
; show up as untyped names — color them as variables.
[
  (each_start_expr)
  (each_end_expr)
  (if_start_expr)
  (if_end_expr)
  (else_if_expr)
  (await_start_expr)
  (await_end_expr)
  (then_expr)
  (catch_expr)
  (key_start_expr)
  (key_end_expr)
  (snippet_start_expr)
  (snippet_end_expr)
  (html_expr)
  (debug_expr)
  (const_expr)
  (render_expr)
  (raw_text_expr)
] @variable

(expr_attribute_value) @variable
(snippet_name) @function
