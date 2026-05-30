;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/scala/locals.scm
;;
;; Vendored verbatim for vorto.

; Scopes
[
  (template_body)
  (lambda_expression)
  (function_definition)
  (block)
  (for_expression)
] @local.scope

; References
(identifier) @local.reference

; Definitions
(function_declaration
  name: (identifier) @local.definition.function)

(function_definition
  name: (identifier) @local.definition.function
  (#set! definition.var.scope parent))

(parameter
  name: (identifier) @local.definition.parameter)

(class_parameter
  name: (identifier) @local.definition.parameter)

(lambda_expression
  parameters: (identifier) @local.definition.var)

(binding
  name: (identifier) @local.definition.var)

(val_definition
  pattern: (identifier) @local.definition.var)

(var_definition
  pattern: (identifier) @local.definition.var)

(val_declaration
  name: (identifier) @local.definition.var)

(var_declaration
  name: (identifier) @local.definition.var)

(for_expression
  enumerators: (enumerators
    (enumerator
      (tuple_pattern
        (identifier) @local.definition.var))))
