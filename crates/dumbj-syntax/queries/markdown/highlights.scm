; Adapted from tree-sitter-md 0.5.4 (MIT), tree-sitter-markdown/queries/highlights.scm.
; https://github.com/tree-sitter-grammars/tree-sitter-markdown
; Block structure only: DumbJ parses one grammar per file, so the inline grammar
; (emphasis, links, code spans) is not applied and paragraphs stay plain.
; Later patterns take precedence over earlier ones.

; Headings
(atx_heading (inline) @keyword)

(setext_heading (paragraph) @keyword)

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.special

; Code
[
  (indented_code_block)
  (code_fence_content)
] @string

(fenced_code_block_delimiter) @punctuation.delimiter

(info_string (language) @label)

; Links and references
(link_destination) @string

(link_label) @label

(link_title) @string

(link_reference_definition ":" @punctuation.delimiter)

; Lists, quotes, rules
[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
  (block_continuation)
  (block_quote_marker)
] @punctuation.special

[
  (task_list_marker_checked)
  (task_list_marker_unchecked)
] @constant.builtin

; Tables
(pipe_table_header (pipe_table_cell) @keyword)

(pipe_table_delimiter_row) @punctuation.delimiter

; Front matter and HTML
[
  (minus_metadata)
  (plus_metadata)
] @comment

(html_block) @comment

; Escapes and entities
(backslash_escape) @string.escape

[
  (entity_reference)
  (numeric_character_reference)
] @constant
