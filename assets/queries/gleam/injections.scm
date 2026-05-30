;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/gleam/injections.scm
;;
;; Vendored verbatim for vorto.

; Comments
([
  (module_comment)
  (statement_comment)
  (comment)
] @injection.content
  (#set! injection.language "comment"))
