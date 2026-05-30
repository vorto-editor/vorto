;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/proto/indents.scm
;;
;; Vendored verbatim for vorto.

[
  (message_body)
  (enum_body)
] @indent.begin

"}" @indent.end @indent.branch

[
  (ERROR)
  (comment)
] @indent.auto
