use dumbj_text::{Buffer, ByteOffset, ByteRange};
use tree_sitter::Node;

use crate::structure::{Source, range_of};
use crate::{Language, SyntaxTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceRole {
    Type,
    /// The callee of a call: `foo()`, `x.foo()`. In Kotlin this may also be a constructor.
    Call,
    Member,
    Value,
    Declaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Qualifier {
    This,
    Super,
    Name(String),
    Path(String),
    /// A call: `foo(x)`, `a.foo(x)`, or in Kotlin a constructor call `Foo(x)`.
    Call(Box<Call>),
    Member(Box<Qualifier>, String),
    /// A value whose type is written on the spot: `new Foo()`, `x as Foo`, or a literal
    /// (`String`, and `Int`/`int` and the like in the file's language).
    Typed(String),
    Lambda,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Call {
    pub receiver: Option<Qualifier>,
    pub name: String,
    pub arguments: Vec<Argument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Argument {
    pub name: Option<String>,
    pub value: Qualifier,
    /// A Kotlin lambda written after the parentheses; it goes to the last parameter.
    pub trailing: bool,
}

impl Qualifier {
    /// `receiver.name`: a longer dotted path when the receiver is one, otherwise a member
    /// read.
    pub(crate) fn member(receiver: Self, name: String) -> Self {
        match receiver {
            Self::Name(first) => Self::Path(format!("{first}.{name}")),
            Self::Path(path) => Self::Path(format!("{path}.{name}")),
            Self::Other | Self::Lambda => Self::Other,
            other => Self::Member(Box::new(other), name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub range: ByteRange,
    pub role: ReferenceRole,
    pub qualifier: Option<Qualifier>,
    pub arguments: Option<Vec<Argument>>,
    pub in_import: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDefinition {
    pub name_range: ByteRange,
    pub type_name: Option<String>,
    pub initialiser: Option<Qualifier>,
}

pub(crate) struct Binder<'t> {
    pub(crate) name: Node<'t>,
    pub(crate) type_name: Option<String>,
    pub(crate) initialiser: Option<Qualifier>,
}

impl<'t> Binder<'t> {
    pub(crate) fn new(name: Node<'t>, type_name: Option<String>) -> Self {
        Self {
            name,
            type_name,
            initialiser: None,
        }
    }
}

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
            Language::Yaml
            | Language::Xml
            | Language::Properties
            | Language::Json
            | Language::Toml
            | Language::Groovy
            | Language::Markdown
            | Language::Shell
            | Language::Dockerfile
            | Language::Sql => (ReferenceRole::Value, None, false),
        };
        let arguments = match (role, self.language()) {
            (ReferenceRole::Call, Language::Java) => crate::java::call_arguments(source, node),
            (ReferenceRole::Call, Language::Kotlin) => crate::kotlin::call_arguments(source, node),
            _ => None,
        };
        Reference {
            name: source.text(node),
            range: range_of(node),
            role,
            qualifier,
            arguments,
            in_import,
        }
    }

    /// `buffer` must hold the text this tree was parsed from.
    #[must_use]
    pub fn reference_at(&self, buffer: &Buffer, offset: ByteOffset) -> Option<Reference> {
        let node = self.identifier_node(offset)?;
        Some(self.classify(&Source { buffer }, node))
    }

    #[must_use]
    pub fn extension_receiver_at(&self, buffer: &Buffer, offset: ByteOffset) -> Option<String> {
        if self.language() != Language::Kotlin {
            return None;
        }
        let node = self.identifier_node(offset)?;
        crate::kotlin::extension_receiver(&Source { buffer }, node)
    }

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
                    Language::Yaml
                    | Language::Xml
                    | Language::Properties
                    | Language::Json
                    | Language::Toml
                    | Language::Groovy
                    | Language::Markdown
                    | Language::Shell
                    | Language::Dockerfile
                    | Language::Sql => Vec::new(),
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
                    initialiser: binder.initialiser,
                });
            }
            inner = scope;
        }
        None
    }
}
