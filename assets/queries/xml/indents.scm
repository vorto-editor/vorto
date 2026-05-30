;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/xml/indents.scm
;;
;; Vendored verbatim for vorto.

(element) @indent.begin

[
  (Attribute)
  (AttlistDecl)
  (contentspec)
] @indent.align

(ETag) @indent.branch

(doctypedecl) @indent.ignore

[
  (Comment)
  (ERROR)
] @indent.auto
