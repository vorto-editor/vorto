;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/ocaml/folds.scm
;;
;; Vendored verbatim for vorto.

[
  (let_binding)
  (external)
  (type_binding)
  (exception_definition)
  (module_binding)
  (module_type_definition)
  (open_module)
  (include_module)
  (include_module_type)
  (class_binding)
  (class_type_binding)
  (value_specification)
  (inheritance_specification)
  (instance_variable_specification)
  (method_specification)
  (inheritance_definition)
  (instance_variable_definition)
  (method_definition)
  (class_initializer)
  (match_case)
  (attribute)
  (item_attribute)
  (floating_attribute)
  (extension)
  (item_extension)
  (quoted_extension)
  (quoted_item_extension)
  (comment)
] @fold
