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

/// Collects symbols while walking; keeps the push logic in one place.
pub(crate) struct Collector<'a> {
    pub(crate) source: Source<'a>,
    pub(crate) facts: FileFacts,
}

impl Collector<'_> {
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
            Language::Yaml => {}
        }
        collector.facts
    }
}
