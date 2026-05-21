; Minimal highlights for tree-sitter-diff. Captures align with the
; Helix-style `diff.*` convention (see src/syntax/theme.rs).

(comment) @comment

(addition) @diff.plus
(new_file) @diff.plus

(deletion) @diff.minus
(old_file) @diff.minus

(commit) @constant
(location) @attribute
(filename) @string
(mode) @number

(command
  "diff" @function
  (argument) @variable.parameter)

(index
  "index" @keyword)

(similarity
  (score) @number)
