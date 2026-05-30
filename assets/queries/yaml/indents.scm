;; YAML indent query.
;;
;; Block collections are the indentation-bearing containers: a
;; `block_mapping` (the `key: value` pairs under a key) and a
;; `block_sequence` (a list of `- item`s) each open one level for the
;; lines they span. Flow collections (`[ ... ]` / `{ ... }`) indent like
;; brackets when written across multiple lines.

[
  (block_mapping)
  (block_sequence)
  (flow_mapping)
  (flow_sequence)
] @indent.begin

[
  "]"
  "}"
] @indent.end

[
  "["
  "]"
  "{"
  "}"
] @indent.branch

(comment) @indent.ignore
