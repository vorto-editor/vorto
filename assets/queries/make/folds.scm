;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/make/folds.scm
;;
;; Vendored verbatim for vorto. (`#trim!` is a no-op in vorto's fold engine.)

([
  (conditional)
  (rule)
  (define_directive)
] @fold
  (#trim! @fold))
