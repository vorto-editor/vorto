;; Based on nvim-treesitter (Apache-2.0).
;; Source: queries/php_only/folds.scm
;;   @ https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/php_only/folds.scm
;;
;; nvim-treesitter's `php/folds.scm` is `; inherits: php_only`. vorto's `php`
;; grammar is the full grammar (php/ subpath), so the php_only rules are
;; resolved inline here.

[
  (if_statement)
  (switch_statement)
  (while_statement)
  (do_statement)
  (for_statement)
  (foreach_statement)
  (try_statement)
  (function_definition)
  (class_declaration)
  (interface_declaration)
  (trait_declaration)
  (enum_declaration)
  (function_static_declaration)
  (method_declaration)
  (namespace_use_declaration)+
] @fold
