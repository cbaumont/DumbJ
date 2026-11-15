; Adapted from tree-sitter-json 0.24.8 (MIT), queries/highlights.scm.
; https://github.com/tree-sitter/tree-sitter-json
; Later patterns take precedence over earlier ones.

(string) @string

(pair
  key: (string) @property)

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(escape_sequence) @string.escape

(comment) @comment

["{" "}" "[" "]"] @punctuation.bracket

["," ":"] @punctuation.delimiter
