;; Based on nvim-treesitter (Apache-2.0).
;; Source: https://github.com/nvim-treesitter/nvim-treesitter/blob/cf12346a3414fa1b06af75c79faebe7f76df080a/queries/cmake/indents.scm
;;
;; Vendored verbatim for vorto.

[
  (normal_command)
  (if_condition)
  (foreach_loop)
  (while_loop)
  (function_def)
  (macro_def)
  (block_def)
] @indent.begin

[
  (elseif_command)
  (else_command)
  (endif_command)
  (endforeach_command)
  (endwhile_command)
  (endfunction_command)
  (endmacro_command)
  (endblock_command)
] @indent.branch

")" @indent.branch

")" @indent.end

(argument_list) @indent.auto
