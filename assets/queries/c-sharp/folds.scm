;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/c_sharp/folds.scm
;;
;; Vendored verbatim for vorto.

body: [
  (declaration_list)
  (switch_body)
  (enum_member_declaration_list)
] @fold

accessors: (accessor_list) @fold

initializer: (initializer_expression) @fold

[
  (block)
  (preproc_if)
  (preproc_elif)
  (preproc_else)
  (using_directive)+
] @fold
