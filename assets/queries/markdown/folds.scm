;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/markdown/folds.scm
;;
;; Vendored verbatim for vorto. (`#trim!` is a no-op in vorto's fold engine —
;; harmless, the fold just may include a trailing blank row.)

([
  (fenced_code_block)
  (indented_code_block)
  (list_item
    (list))
  (section)
] @fold
  (#trim! @fold))

(section
  (list) @fold
  (#trim! @fold))
