; Multi-line tags and Svelte's block statements act as indent scopes.
; Elements and script/style cover the HTML half; the `*_statement`
; nodes cover `{#if}` / `{#each}` / `{#await}` / `{#key}` /
; `{#snippet}` so guides connect through their block bodies.

[
  (element)
  (script_element)
  (style_element)
  (if_statement)
  (each_statement)
  (await_statement)
  (key_statement)
  (snippet_statement)
] @indent.begin

(comment) @indent.ignore
