; Static-language injections for Svelte. Patterns that need
; `@injection.language` capture-driven resolution are omitted — the
; engine only handles `(#set! injection.language "X")` for now.

; `<script>` (no lang attribute) → JavaScript.
((script_element
  (start_tag) @_tag
  (raw_text) @injection.content)
  (#not-match? @_tag "lang=")
  (#set! injection.language "javascript"))

; `<script lang="ts">` (or `lang="typescript"`) → TypeScript.
((script_element
  (start_tag
    (attribute
      (attribute_name) @_lang_attr
      (quoted_attribute_value
        (attribute_value) @_lang)))
  (raw_text) @injection.content)
  (#eq? @_lang_attr "lang")
  (#match? @_lang "^(ts|typescript)$")
  (#set! injection.language "typescript"))

; `<style>` → CSS.
((style_element
  (raw_text) @injection.content)
  (#set! injection.language "css"))

; Mustache-style `{expression}` regions are parsed by the svelte
; grammar as `raw_text_expr`; treat the expression as JavaScript.
((raw_text_expr) @injection.content
  (#set! injection.language "javascript"))
