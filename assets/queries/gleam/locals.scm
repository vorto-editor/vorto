;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/gleam/locals.scm
;;
;; Vendored verbatim for vorto.

; Let Binding Definition
(let
  pattern: (identifier) @local.definition)

; List Pattern Definitions
(list_pattern
  (identifier) @local.definition)

(list_pattern
  assign: (identifier) @local.definition)

; Tuple Pattern Definition
(tuple_pattern
  (identifier) @local.definition)

; Record Pattern Definition
(record_pattern_argument
  pattern: (identifier) @local.definition)

; Function Parameter Definition
(function_parameter
  name: (identifier) @local.definition)

; References
(identifier) @local.reference

; Block Scope
(block) @local.scope

; Case Scope
(case_clause) @local.scope
