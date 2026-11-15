; Adapted from tree-sitter-properties 0.3.0 (MIT), queries/highlights.scm.
; https://github.com/tree-sitter-grammars/tree-sitter-properties
; Later patterns take precedence over earlier ones.

(comment) @comment

(key) @property

(value) @string

(value (escape) @string.escape)

((index) @number
  (#match? @number "^[0-9]+$"))

((substitution (key) @constant)
  (#match? @constant "^[A-Z0-9_]+"))

(substitution
  (key) @function
  "::" @punctuation.special
  (secret) @string)

(property ["=" ":"] @operator)

["${" "}"] @punctuation.special

(substitution ":" @punctuation.special)

["[" "]"] @punctuation.bracket

["." "\\"] @punctuation.delimiter
