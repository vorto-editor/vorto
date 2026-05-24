;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/php_only/injections.scm
;;
;; Modification: #lua-match? predicates rewritten as #match? for tree-sitter native engine.

((comment) @injection.content
  (#set! injection.language "phpdoc"))

(heredoc
  (heredoc_body) @injection.content
  (heredoc_end) @injection.language
  (#set! injection.include-children)
  (#downcase! @injection.language))

(nowdoc
  (nowdoc_body) @injection.content
  (heredoc_end) @injection.language
  (#set! injection.include-children)
  (#downcase! @injection.language))

; regex
((function_call_expression
  function: (_) @_preg_func_identifier
  arguments: (arguments
    .
    (argument
      (_
        (string_content) @injection.content))))
  (#set! injection.language "regex")
  (#match? @_preg_func_identifier "^preg_"))

; bash
((function_call_expression
  function: (_) @_shell_func_identifier
  arguments: (arguments
    .
    (argument
      (_
        (string_content) @injection.content))))
  (#set! injection.language "bash")
  (#any-of? @_shell_func_identifier
    "shell_exec" "escapeshellarg" "escapeshellcmd" "exec" "passthru" "proc_open" "shell_exec"
    "system"))

(expression_statement
  (shell_command_expression
    (string_content) @injection.content)
  (#set! injection.language "bash"))
