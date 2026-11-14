//! What an identifier under the caret refers to syntactically, and resolution of names
//! declared in enclosing local scopes (parameters, local variables, type parameters).

use dumbj_text::{Buffer, ByteOffset, ByteRange};
use tree_sitter::Node;

use crate::structure::{Source, range_of};
use crate::{Language, SyntaxTree};

/// How an identifier is used where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceRole {
    /// A type position: `Foo x`, `: Foo`, `new Foo()`, an annotation, an import.
    Type,
    /// The callee of a call: `foo()`, `x.foo()`. In Kotlin this may also be a constructor.
    Call,
    /// A member read after a dot: `x.foo`.
    Member,
    /// A bare name in an expression: a local, a field, or a type used as a qualifier.
    Value,
    /// The name being declared; the caret is already on a declaration.
    Declaration,
}

/// What stands before the dot of a qualified reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Qualifier {
    This,
    Super,
    /// A single identifier: a local, a field or a type name.
    Name(String),
    /// Dotted identifiers such as `java.util` or `a.b`.
    Path(String),
    /// Any other expression, e.g. a call. Its type is unknown to the dumb engine.
    Other,
}

/// An identifier occurrence and its syntactic context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub range: ByteRange,
    pub role: ReferenceRole,
    pub qualifier: Option<Qualifier>,
    /// Whether the identifier is part of an import statement.
    pub in_import: bool,
}

/// A parameter, local variable or type parameter that a reference resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDefinition {
    pub name_range: ByteRange,
    /// The declared type without type arguments, or the constructor name of an obvious
    /// initialiser (`val x = Foo()`), when either is available.
    pub type_name: Option<String>,
}

/// A name introduced into a scope, with the node that carries its type if there is one.
pub(crate) struct Binder<'t> {
    pub(crate) name: Node<'t>,
    pub(crate) type_name: Option<String>,
}

/// Classifies a qualifier expression by its text.
pub(crate) fn qualifier_from_text(text: &str) -> Qualifier {
    let is_identifier = |part: &str| {
        let mut chars = part.chars();
        chars
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    };
    match text {
        "this" => Qualifier::This,
        "super" => Qualifier::Super,
        _ if is_identifier(text) => Qualifier::Name(text.to_owned()),
        _ if text.split('.').all(is_identifier) => Qualifier::Path(text.to_owned()),
        _ => Qualifier::Other,
    }
}

fn is_identifier_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier" | "simple_identifier")
}

impl SyntaxTree {
    /// The identifier node at `offset`, also trying the character just before it so a
    /// caret at the end of a word still counts.
    fn identifier_node(&self, offset: ByteOffset) -> Option<Node<'_>> {
        let root = self.tree().root_node();
        let at = |position: usize| {
            root.named_descendant_for_byte_range(position, position)
                .filter(|node| is_identifier_kind(node.kind()))
        };
        at(offset.get()).or_else(|| at(offset.get().checked_sub(1)?))
    }

    fn classify(&self, source: &Source<'_>, node: Node<'_>) -> Reference {
        let (role, qualifier, in_import) = match self.language() {
            Language::Java => crate::java::classify(source, node),
            Language::Kotlin => crate::kotlin::classify(source, node),
            Language::Yaml => (ReferenceRole::Value, None, false),
        };
        Reference {
            name: source.text(node),
            range: range_of(node),
            role,
            qualifier,
            in_import,
        }
    }

    /// Describes the identifier at `offset`, or `None` if there is no identifier there
    /// (whitespace, a keyword, a comment, a string).
    ///
    /// `buffer` must hold the text this tree was parsed from.
    #[must_use]
    pub fn reference_at(&self, buffer: &Buffer, offset: ByteOffset) -> Option<Reference> {
        let node = self.identifier_node(offset)?;
        Some(self.classify(&Source { buffer }, node))
    }

    /// Every identifier in the file spelled `name`, in source order. Comments and strings
    /// are never included because they contain no identifier nodes.
    #[must_use]
    pub fn references_named(&self, buffer: &Buffer, name: &str) -> Vec<Reference> {
        let source = Source { buffer };
        let mut found = Vec::new();
        let mut cursor = self.tree().walk();
        // Pre-order walk without recursion: files can be deeply nested.
        loop {
            let node = cursor.node();
            if is_identifier_kind(node.kind())
                && node.byte_range().len() == name.len()
                && source.text(node) == name
            {
                found.push(self.classify(&source, node));
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return found;
                }
            }
        }
    }

    /// Resolves the identifier at `offset` to a declaration in an enclosing local scope: a
    /// parameter, a local variable, a loop or catch variable, or (for `Type` references)
    /// a type parameter. The innermost, nearest preceding declaration wins.
    ///
    /// Returns `None` when the name is not declared locally, which means it has to be
    /// looked up among fields, types and imports instead.
    #[must_use]
    pub fn resolve_local(
        &self,
        buffer: &Buffer,
        offset: ByteOffset,
        name: &str,
        role: ReferenceRole,
    ) -> Option<LocalDefinition> {
        let source = Source { buffer };
        let start = self.identifier_node(offset)?;
        let reference_start = start.start_byte();
        let want_type = role == ReferenceRole::Type;
        let mut inner = start;
        while let Some(scope) = inner.parent() {
            let mut cursor = scope.walk();
            let mut best: Option<Binder<'_>> = None;
            for child in scope.children(&mut cursor) {
                if child.start_byte() >= inner.start_byte() {
                    break;
                }
                let binders = match self.language() {
                    Language::Java => crate::java::binders(&source, scope, child, want_type),
                    Language::Kotlin => crate::kotlin::binders(&source, scope, child, want_type),
                    Language::Yaml => Vec::new(),
                };
                for binder in binders {
                    if binder.name.start_byte() < reference_start
                        && source.text(binder.name) == name
                    {
                        best = Some(binder);
                    }
                }
            }
            if let Some(binder) = best {
                return Some(LocalDefinition {
                    name_range: range_of(binder.name),
                    type_name: binder.type_name,
                });
            }
            inner = scope;
        }
        None
    }
}
