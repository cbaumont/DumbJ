; Adapted from tree-sitter-xml 0.7.0 (MIT), queries/xml/highlights.scm.
; https://github.com/tree-sitter-grammars/tree-sitter-xml
; DumbJ has no `tag` or `markup` roles: tag names use the keyword colour and
; attribute names the property colour. Later patterns take precedence.

;; XML declaration

"xml" @keyword

["version" "encoding" "standalone"] @property

(EncName) @string

(VersionNum) @number

["yes" "no"] @boolean

;; Processing instructions

(PI (PITarget) @keyword)

(XmlModelPI "xml-model" @keyword)

(StyleSheetPI "xml-stylesheet" @keyword)

(PseudoAtt (Name) @property)

(PseudoAtt (PseudoAttValue) @string)

;; Doctype and DTD declarations

(doctypedecl "DOCTYPE" @keyword)

(doctypedecl (Name) @type)

(elementdecl
  "ELEMENT" @keyword
  (Name) @keyword)

(contentspec
  (_ (Name) @property))

"#PCDATA" @type.builtin

["EMPTY" "ANY"] @constant.builtin

["*" "?" "+"] @operator

(GEDecl
  "ENTITY" @keyword
  (Name) @constant)

(GEDecl (EntityValue) @string)

(PEDecl
  "ENTITY" @keyword
  "%" @operator
  (Name) @constant)

(PEDecl (EntityValue) @string)

(NDataDecl
  "NDATA" @keyword
  (Name) @label)

(NotationDecl
  "NOTATION" @keyword
  (Name) @constant)

(AttlistDecl
  "ATTLIST" @keyword
  (Name) @keyword)

(AttDef (Name) @property)

(AttDef (Enumeration (Nmtoken) @string))

(DefaultDecl (AttValue) @string)

[(StringType) (TokenizedType)] @type.builtin

(NotationType "NOTATION" @type.builtin)

["#REQUIRED" "#IMPLIED" "#FIXED"] @attribute

["PUBLIC" "SYSTEM"] @keyword

(PubidLiteral) @string

(SystemLiteral (URI) @string)

;; Entities

(EntityRef) @constant

((EntityRef) @constant.builtin
  (#any-of? @constant.builtin "&amp;" "&lt;" "&gt;" "&quot;" "&apos;"))

(CharRef) @constant

(PEReference) @constant

;; Elements and attributes

(STag (Name) @keyword)

(ETag (Name) @keyword)

(EmptyElemTag (Name) @keyword)

(Attribute (Name) @property)

(Attribute (AttValue) @string)

(CDSect (CData) @string)

;; Punctuation

["<?" "?>" "<!" "]]>" "<" ">" "</" "/>"] @punctuation.delimiter

["(" ")" "[" "]"] @punctuation.bracket

["\"" "'"] @punctuation.delimiter

["," "|" "="] @operator

(Comment) @comment
