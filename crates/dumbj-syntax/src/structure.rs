//! The declarations, package and imports of a source file: the facts the symbol index and
//! File Structure are built from.

use dumbj_text::{Buffer, ByteOffset, ByteRange};
use tree_sitter::Node;

use crate::{Language, SyntaxTree};

/// What kind of declaration a [`Symbol`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    Class,
    Interface,
    Enum,
    Record,
    Annotation,
    /// A Kotlin `object`.
    Object,
    /// A Kotlin `companion object`.
    Companion,
    TypeAlias,
    /// A function outside any type (Kotlin top level).
    Function,
    /// A function declared in a type.
    Method,
    Constructor,
    Field,
    /// A Kotlin property, including `val`/`var` constructor parameters.
    Property,
    EnumConstant,
}

impl SymbolKind {
    /// Every kind, for tests and exhaustive tables.
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
    ];

    /// A stable name for storing the kind outside the process; see [`Self::from_name`].
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
        }
    }

    /// The kind stored under `name` by [`Self::as_str`], or `None` for an unknown name.
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
            _ => None,
        }
    }
    /// Whether the symbol declares a type that can contain members.
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

    /// Whether the symbol can be called.
    #[must_use]
    pub fn is_callable(self) -> bool {
        matches!(self, Self::Function | Self::Method | Self::Constructor)
    }
}

/// One declaration in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// The whole declaration.
    pub range: ByteRange,
    /// The declared name; where navigation lands.
    pub name_range: ByteRange,
    /// The index, in [`FileFacts::symbols`], of the enclosing declaration.
    pub parent: Option<usize>,
    /// Supertypes as written, without type arguments (`Runnable`, `java.io.Serializable`).
    pub supertypes: Vec<String>,
    /// The receiver type of a Kotlin extension function or property, as written.
    pub receiver: Option<String>,
    /// The declared type of a field or property, or the return type of a function, without
    /// type arguments. `None` when it is not written, is primitive or is `void`.
    pub type_name: Option<String>,
    /// The parameters of a function, method or constructor, or of the primary constructor
    /// of a Kotlin class (empty when the class declares no constructor at all). `None`
    /// for everything that cannot be called as written.
    pub parameters: Option<Vec<Parameter>>,
    /// Which side of a Kotlin multiplatform `expect`/`actual` pair this is, if any.
    pub platform: Option<Platform>,
}

/// A side of a Kotlin multiplatform declaration pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// `expect`: the declaration common code sees. Members of an `expect` type are too.
    Expect,
    /// `actual`: the implementation in one platform's source set.
    Actual,
}

/// One parameter of a callable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    /// The declared type without type arguments. `None` when it is a type parameter, a
    /// function type or a primitive Java type.
    pub type_name: Option<String>,
    /// Whether a call may leave it out: it has a default value.
    pub optional: bool,
    /// `vararg` or `...`: takes any number of arguments.
    pub variadic: bool,
}

/// One import statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The imported name without any trailing `.*`, e.g. `java.util.List` or `java.util`.
    pub path: String,
    pub wildcard: bool,
    /// A Java `import static`.
    pub is_static: bool,
    /// The Kotlin `as` name.
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

/// Everything the index needs to know about one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub package: Option<String>,
    pub imports: Vec<Import>,
    /// Declarations in source order; a parent always comes before its children.
    pub symbols: Vec<Symbol>,
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

/// Shared helpers for walking a tree with its source text.
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

/// The named children of `node`.
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

/// Collects symbols while walking; keeps the push logic in one place.
pub(crate) struct Collector<'a> {
    pub(crate) source: Source<'a>,
    pub(crate) facts: FileFacts,
}

impl Collector<'_> {
    /// Sets the declared type of the symbol at `index`.
    pub(crate) fn set_type(&mut self, index: usize, type_name: Option<String>) {
        if let Some(symbol) = self.facts.symbols.get_mut(index) {
            symbol.type_name = type_name;
        }
    }

    /// Sets the parameters of the callable at `index`.
    pub(crate) fn set_parameters(&mut self, index: usize, parameters: Option<Vec<Parameter>>) {
        if let Some(symbol) = self.facts.symbols.get_mut(index) {
            symbol.parameters = parameters;
        }
    }

    /// Adds a symbol and returns its index.
    pub(crate) fn push(
        &mut self,
        kind: SymbolKind,
        declaration: Node<'_>,
        name: Node<'_>,
        parent: Option<usize>,
    ) -> usize {
        self.facts.symbols.push(Symbol {
            name: self.source.text(name),
            kind,
            range: range_of(declaration),
            name_range: range_of(name),
            parent,
            supertypes: Vec::new(),
            receiver: None,
            type_name: None,
            parameters: None,
            platform: None,
        });
        self.facts.symbols.len() - 1
    }
}

impl SyntaxTree {
    /// Extracts the package, imports and declarations of the file.
    ///
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
            Language::Yaml | Language::Xml | Language::Properties | Language::Json => {}
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
