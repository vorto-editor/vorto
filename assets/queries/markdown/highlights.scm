; Originally derived from nvim-treesitter/nvim-treesitter, migrated to
; the Helix-style `markup.*` capture namespace. The whole-line
; `markup.heading` capture paints the heading band (bg + fg + bold);
; the marker capture layers a magenta fg on top via the renderer's
; style-patching behavior.
(atx_heading) @markup.heading
(setext_heading) @markup.heading

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @markup.heading.marker

[
  (link_title)
  (indented_code_block)
  (fenced_code_block)
] @markup.raw

(fenced_code_block_delimiter) @punctuation.delimiter

(code_fence_content) @none

(link_destination) @markup.link.url

(link_label) @markup.link.label

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
] @markup.list

[
  (block_continuation)
  (block_quote_marker)
] @punctuation.special

(backslash_escape) @string.escape
