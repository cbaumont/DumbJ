; Adapted from tree-sitter-bash 0.25.1 (MIT), queries/highlights.scm.
; https://github.com/tree-sitter/tree-sitter-bash
; Extended with variables, numbers, redirects and test operators. Later patterns take
; precedence over earlier ones.

(comment) @comment

[
  (string)
  (raw_string)
  (ansi_c_string)
  (heredoc_body)
  (heredoc_start)
] @string

(heredoc_end) @string

(command_name (word) @function)

(function_definition name: (word) @function)

[
  "case"
  "do"
  "done"
  "elif"
  "else"
  "esac"
  "export"
  "fi"
  "for"
  "function"
  "if"
  "in"
  "select"
  "then"
  "unset"
  "until"
  "while"
  "declare"
  "local"
  "readonly"
  "typeset"
] @keyword

["(" ")" "[[" "]]" "[" "]" "{" "}"] @punctuation.bracket

; Variables: assignments, uses and expansions.
(variable_name) @property

(special_variable_name) @variable.builtin

(simple_expansion "$" @punctuation.special)

(expansion
  ["${" "}"] @punctuation.special)

(command_substitution
  ["$(" ")"] @punctuation.special)

(file_descriptor) @number

(number) @number

[
  "$"
  "&&"
  "||"
  "|"
  "|&"
  "!"
  ">"
  ">>"
  "<"
  "<<"
  "<<<"
  "&>"
  "&>>"
  ">&"
  "="
  "+="
  "=~"
  "=="
  "!="
] @operator

[";" ";;" "&"] @punctuation.delimiter

; Options such as -x or --verbose.
(
  (command (_) @constant)
  (#match? @constant "^-")
)

(test_operator) @operator
