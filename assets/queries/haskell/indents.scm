; Haskell indent query. Hand-authored for vorto's `@indent.begin`
; convention (nvim-treesitter ships no haskell `indents.scm` at the pinned
; commit). Haskell is layout-sensitive: top-level declarations sit at
; column 0, so the useful indent scopes are the *indented* bodies — `do`
; blocks, `where` / `let` binding groups (`local_binds`), `case`
; alternatives, and record `fields`. The module-level `declarations` node
; is deliberately NOT captured: it spans the whole file at column 0 and
; would mask every finer scope (and vorto's whitespace-based guide
; fallback) behind a column-0 scope that never renders a guide.

[
  (do)
  (alternatives)
  (local_binds)
  (let)
  (fields)
] @indent.begin

[
  (comment)
  (string)
  (quasiquote)
] @indent.ignore
