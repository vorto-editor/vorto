; Static-language injections for Vue. Patterns that need
; `@injection.language` capture-driven resolution (e.g. `<script
; lang="$attr">` where the language name comes from the attribute
; value) are omitted — the engine only handles `(#set!
; injection.language "X")` for now.

; `<script>` (no lang attribute) → JavaScript.
((script_element
  (start_tag) @_tag
  (raw_text) @injection.content)
  (#not-match? @_tag "lang=")
  (#set! injection.language "javascript"))

; `<script lang="js">` → JavaScript.
((script_element
  (start_tag
    (attribute
      (attribute_name) @_lang_attr
      (quoted_attribute_value
        (attribute_value) @_lang)))
  (raw_text) @injection.content)
  (#eq? @_lang_attr "lang")
  (#eq? @_lang "js")
  (#set! injection.language "javascript"))

; `<script lang="ts">` → TypeScript.
((script_element
  (start_tag
    (attribute
      (attribute_name) @_lang_attr
      (quoted_attribute_value
        (attribute_value) @_lang)))
  (raw_text) @injection.content)
  (#eq? @_lang_attr "lang")
  (#eq? @_lang "ts")
  (#set! injection.language "typescript"))

; `<style>` (with or without scoped / lang attrs) → CSS. The other
; preprocessors (`scss` / `sass` / `less`) would need their own
; grammars; until those are installed they fall through to plain CSS,
; which is wrong but visually close enough.
((style_element
  (raw_text) @injection.content)
  (#set! injection.language "css"))
