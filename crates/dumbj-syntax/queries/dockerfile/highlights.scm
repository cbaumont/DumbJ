; Adapted from tree-sitter-dockerfile-updated 0.2.0 (MIT), queries/highlights.scm.
; https://github.com/camdencheek/tree-sitter-dockerfile (grammar by camdencheek)
; Later patterns take precedence over earlier ones.

[
  "FROM"
  "AS"
  "RUN"
  "CMD"
  "LABEL"
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
  "MAINTAINER"
  "CROSS_BUILD"
  (heredoc_marker)
  (heredoc_end)
] @keyword

(image_name) @type

(image_alias) @label

(image_tag) @string

(image_digest) @string

(image_spec
  (image_tag ":" @punctuation.special)
  (image_digest "@" @punctuation.special))

(env_pair name: (_) @property)

(label_pair key: (_) @property)

(expose_port) @number

(param) @attribute

(mount_param) @attribute

[
  (double_quoted_string)
  (single_quoted_string)
  (json_string)
  (heredoc_line)
] @string

(escape_sequence) @string.escape

(expansion
  ["$" "{" "}"] @punctuation.special)

(variable) @variable

((variable) @constant
  (#match? @constant "^[A-Z][A-Z_0-9]*$"))

(line_continuation) @punctuation.delimiter

(comment) @comment
