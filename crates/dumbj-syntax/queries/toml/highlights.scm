; Adapted from tree-sitter-toml-ng 0.7.0 (MIT), queries/highlights.scm.
; https://github.com/tree-sitter-grammars/tree-sitter-toml

(bare_key) @property

(quoted_key) @property

(table [(bare_key) (quoted_key) (dotted_key (bare_key))] @type)

(table_array_element [(bare_key) (quoted_key) (dotted_key (bare_key))] @type)

(boolean) @boolean

(comment) @comment

(string) @string

(escape_sequence) @string.escape

[
  (integer)
  (float)
] @number

[
  (offset_date_time)
  (local_date_time)
  (local_date)
  (local_time)
] @string

[
  "."
  ","
] @punctuation.delimiter

"=" @operator

[
  "["
  "]"
  "[["
  "]]"
  "{"
  "}"
] @punctuation.bracket
