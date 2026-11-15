use dumbj_text::{Buffer, ByteOffset, ByteRange};
use tree_sitter::Node;

use crate::{Language, SyntaxTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
    Object,
    Companion,
    TypeAlias,
    Function,
    Method,
    Constructor,
    Field,
    Property,
    EnumConstant,
    Heading,
    Table,
    Element,
    Key,
}

impl SymbolKind {
    pub const ALL: &[Self] = &[
        Self::Class,
        Self::Interface,
        Self::Enum,
        Self::Record,
        Self::Annotation,
        Self::Object,
        Self::Companion,
        Self::TypeAlias,
        Self::Function,
        Self::Method,
        Self::Constructor,
        Self::Field,
        Self::Property,
        Self::EnumConstant,
        Self::Heading,
        Self::Table,
        Self::Element,
        Self::Key,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "Class",
            Self::Interface => "Interface",
            Self::Enum => "Enum",
            Self::Record => "Record",
            Self::Annotation => "Annotation",
            Self::Object => "Object",
            Self::Companion => "Companion",
            Self::TypeAlias => "TypeAlias",
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Constructor => "Constructor",
            Self::Field => "Field",
            Self::Property => "Property",
            Self::EnumConstant => "EnumConstant",
            Self::Heading => "Heading",
            Self::Table => "Table",
            Self::Element => "Element",
            Self::Key => "Key",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Class" => Some(Self::Class),
            "Interface" => Some(Self::Interface),
            "Enum" => Some(Self::Enum),
            "Record" => Some(Self::Record),
            "Annotation" => Some(Self::Annotation),
            "Object" => Some(Self::Object),
            "Companion" => Some(Self::Companion),
            "TypeAlias" => Some(Self::TypeAlias),
            "Function" => Some(Self::Function),
            "Method" => Some(Self::Method),
            "Constructor" => Some(Self::Constructor),
            "Field" => Some(Self::Field),
            "Property" => Some(Self::Property),
            "EnumConstant" => Some(Self::EnumConstant),
            "Heading" => Some(Self::Heading),
            "Table" => Some(Self::Table),
            "Element" => Some(Self::Element),
            "Key" => Some(Self::Key),
            _ => None,
        }
    }
    #[must_use]
    pub fn is_type(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Interface
                | Self::Enum
                | Self::Record
                | Self::Annotation
                | Self::Object
                | Self::Companion
                | Self::TypeAlias
        )
    }

    /// Whether the symbol is an entry of a document or configuration file (a heading, a
    /// table, a key) rather than a declaration in code. These are shown in File Structure
    /// but are not searched for across the project and never resolve a reference.
    #[must_use]
    pub fn is_document(self) -> bool {
        matches!(
            self,
            Self::Heading | Self::Table | Self::Element | Self::Key
        )
    }

    #[must_use]
    pub fn is_callable(self) -> bool {
        matches!(self, Self::Function | Self::Method | Self::Constructor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub range: ByteRange,
    pub name_range: ByteRange,
    /// The index, in [`FileFacts::symbols`], of the enclosing declaration.
    pub parent: Option<usize>,
    pub supertypes: Vec<String>,
    pub receiver: Option<String>,
    /// The declared type of a field or property, or the return type of a function, without
    /// type arguments. `None` when it is not written, is primitive or is `void`.
    pub type_name: Option<String>,
    /// The parameters of a function, method or constructor, or of the primary constructor
    /// of a Kotlin class (empty when the class declares no constructor at all). `None`
    /// for everything that cannot be called as written.
    pub parameters: Option<Vec<Parameter>>,
    pub platform: Option<Platform>,
    pub jvm_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// `expect`: the declaration common code sees. Members of an `expect` type are too.
    Expect,
    Actual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    /// The declared type without type arguments. `None` when it is a type parameter, a
    /// function type or a primitive Java type.
    pub type_name: Option<String>,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The imported name without any trailing `.*`, e.g. `java.util.List` or `java.util`.
    pub path: String,
    pub wildcard: bool,
    pub is_static: bool,
    pub alias: Option<String>,
}

impl Import {
    /// The simple name this import brings into scope, or `None` for a wildcard.
    #[must_use]
    pub fn local_name(&self) -> Option<&str> {
        if self.wildcard {
            return None;
        }
        self.alias
            .as_deref()
            .or_else(|| self.path.rsplit('.').next())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub package: Option<String>,
    pub imports: Vec<Import>,
    /// Declarations in source order; a parent always comes before its children.
    pub symbols: Vec<Symbol>,
    pub jvm_name: Option<String>,
}

impl FileFacts {
    /// The dotted path of enclosing declarations and the symbol itself, e.g. `Outer.Inner.go`.
    #[must_use]
    pub fn qualified_name(&self, index: usize) -> String {
        let mut names = Vec::new();
        let mut current = Some(index);
        while let Some(symbol) = current.and_then(|index| self.symbols.get(index)) {
            names.push(symbol.name.as_str());
            current = symbol.parent;
        }
        names.reverse();
        names.join(".")
    }
}

pub(crate) struct Source<'a> {
    pub(crate) buffer: &'a Buffer,
}

impl Source<'_> {
    pub(crate) fn text(&self, node: Node<'_>) -> String {
        self.buffer.text(range_of(node)).unwrap_or_default()
    }
}

pub(crate) fn range_of(node: Node<'_>) -> ByteRange {
    ByteOffset(node.start_byte())..ByteOffset(node.end_byte())
}

pub(crate) fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// All children of `node`, including anonymous tokens such as `interface` or `static`.
pub(crate) fn all_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

pub(crate) fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_children(node)
        .into_iter()
        .find(|child| child.kind() == kind)
}

/// Whether `name` is a type parameter of `node` or of a declaration enclosing it. A type
/// parameter is not a type anyone can navigate to or look members up in.
pub(crate) fn is_type_parameter(source: &Source<'_>, node: Node<'_>, name: &str) -> bool {
    std::iter::successors(Some(node), Node::parent)
        .filter_map(|scope| child_of_kind(scope, "type_parameters"))
        .flat_map(named_children)
        .filter_map(|parameter| child_of_kind(parameter, "type_identifier"))
        .any(|parameter| source.text(parameter) == name)
}

pub(crate) struct Collector<'a> {
    pub(crate) source: Source<'a>,
    pub(crate) facts: FileFacts,
}

impl Collector<'_> {
    pub(crate) fn set_type(&mut self, index: usize, type_name: Option<String>) {
        if let Some(symbol) = self.facts.symbols.get_mut(index) {
            symbol.type_name = type_name;
        }
    }

    pub(crate) fn set_parameters(&mut self, index: usize, parameters: Option<Vec<Parameter>>) {
        if let Some(symbol) = self.facts.symbols.get_mut(index) {
            symbol.parameters = parameters;
        }
    }

    pub(crate) fn push(
        &mut self,
        kind: SymbolKind,
        declaration: Node<'_>,
        name: Node<'_>,
        parent: Option<usize>,
    ) -> usize {
        self.push_range(kind, range_of(declaration), range_of(name), parent)
    }

    pub(crate) fn push_range(
        &mut self,
        kind: SymbolKind,
        declaration: ByteRange,
        name_range: ByteRange,
        parent: Option<usize>,
    ) -> usize {
        let name = self
            .source
            .buffer
            .text(name_range.clone())
            .unwrap_or_default();
        self.push_with_name(kind, declaration, name, name_range, parent)
    }

    /// Adds a symbol named `name`, which need not be written anywhere as such (a Maven
    /// `groupId:artifactId`); `landing` is where navigation goes, possibly an empty range.
    pub(crate) fn push_with_name(
        &mut self,
        kind: SymbolKind,
        declaration: ByteRange,
        name: String,
        name_range: ByteRange,
        parent: Option<usize>,
    ) -> usize {
        self.facts.symbols.push(Symbol {
            name,
            kind,
            range: declaration,
            name_range,
            parent,
            supertypes: Vec::new(),
            receiver: None,
            type_name: None,
            parameters: None,
            platform: None,
            jvm_name: None,
        });
        self.facts.symbols.len() - 1
    }
}

impl SyntaxTree {
    /// `buffer` must hold the text this tree was parsed from. Declarations inside function
    /// bodies are left out. Syntax errors are tolerated: whatever still parses is reported.
    #[must_use]
    pub fn facts(&self, buffer: &Buffer) -> FileFacts {
        let mut collector = Collector {
            source: Source { buffer },
            facts: FileFacts::default(),
        };
        let root = self.tree().root_node();
        match self.language() {
            Language::Java => crate::java::collect(&mut collector, root, None),
            Language::Kotlin => crate::kotlin::collect(&mut collector, root, None),
            Language::Markdown => crate::document::markdown(&mut collector, root),
            Language::Toml => crate::document::toml(&mut collector, root),
            Language::Xml => crate::document::xml(&mut collector, root),
            Language::Yaml => crate::document::yaml(&mut collector, root),
            Language::Properties
            | Language::Json
            | Language::Groovy
            | Language::Shell
            | Language::Dockerfile
            | Language::Sql => {}
        }
        collector.facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_kind_round_trips_through_its_name() {
        for &kind in SymbolKind::ALL {
            assert_eq!(SymbolKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(SymbolKind::from_name("Nope"), None);
    }
}
