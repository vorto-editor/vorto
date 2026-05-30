;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/julia/injections.scm
;;
;; Vendored verbatim — predicates (#any-of?, #eq?, #has-ancestor?) and
;; indent directives (#set! indent.*) are supported by vorto's engine as-is.

; Inject markdown in docstrings
((string_literal
  (content) @injection.content)
  .
  [
    (module_definition)
    (abstract_definition)
    (struct_definition)
    (function_definition)
    (macro_definition)
    (assignment)
    (const_statement)
    (call_expression)
    (identifier)
  ]
  (#set! injection.language "markdown"))

; Inject comments
([
  (line_comment)
  (block_comment)
] @injection.content
  (#set! injection.language "comment"))

; Inject regex in r"..." and r"""...""" (e.g. r"hello\bworld")
(prefixed_string_literal
  prefix: (identifier) @_prefix
  (content) @injection.content
  (#eq? @_prefix "r")
  (#set! injection.language "regex"))

; Inject markdown in md"..." and md"""...""" (e.g. md"**Bold** and _Italics_")
(prefixed_string_literal
  prefix: (identifier) @_prefix
  (content) @injection.content
  (#eq? @_prefix "md")
  (#set! injection.language "markdown"))

; Inject bash in `...` and ```...``` (e.g. `git add --help`)
(command_literal
  (content) @injection.content
  (#set! injection.language "bash"))
