; Minimal highlights for tree-sitter-dockerfile. Capture names follow
; the rest of vorto's queries (see src/syntax/theme.rs).

[
  "FROM"
  "RUN"
  "CMD"
  "LABEL"
  "MAINTAINER"
  "EXPOSE"
  "ENV"
  "ADD"
  "COPY"
  "ENTRYPOINT"
  "VOLUME"
  "USER"
  "WORKDIR"
  "ARG"
  "ONBUILD"
  "STOPSIGNAL"
  "HEALTHCHECK"
  "SHELL"
  "AS"
] @keyword

(comment) @comment
(image_spec) @type
(image_tag) @string
(image_digest) @string
(param) @variable.parameter
(mount_param) @variable.parameter

(expansion
  [
    "$"
    "{"
    "}"
  ] @punctuation.special) @none

(variable) @variable

[
  (double_quoted_string)
  (single_quoted_string)
  (json_string)
] @string

(env_pair name: (unquoted_string) @variable)
(label_pair key: (unquoted_string) @attribute)
