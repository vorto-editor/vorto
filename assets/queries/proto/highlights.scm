;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/proto/highlights.scm
;;
;; Modification: `#lua-match?` predicates rewritten as `#match?` (the
;; native engine does not evaluate the Neovim-specific `#lua-match?`); the
;; patterns are already valid Rust regexes, so only the predicate name changed.

[
  "extend"
  "extensions"
  "oneof"
  "option"
  "reserved"
  "syntax"
  "to"
] @keyword

[
  "enum"
  "service"
  "message"
] @keyword.type

"rpc" @keyword.function

"returns" @keyword.return

[
  "optional"
  "repeated"
  "required"
] @keyword.modifier

[
  "package"
  "import"
] @keyword.import

[
  (key_type)
  (type)
  (message_name)
  (enum_name)
  (service_name)
  (rpc_name)
  (message_or_enum_type)
] @type

(enum_field
  (identifier) @constant)

(string) @string

[
  "\"proto3\""
  "\"proto2\""
] @string.special

(int_lit) @number

(float_lit) @number.float

[
  (true)
  (false)
] @boolean

(comment) @comment @spell

((comment) @comment.documentation
  (#match? @comment.documentation "(?s)^/[*][*][^*].*[*]/$"))

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "<"
  ">"
] @punctuation.bracket

[
  ";"
  ","
] @punctuation.delimiter

"=" @operator
