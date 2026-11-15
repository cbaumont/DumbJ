//! Tree-sitter layer with no UI and no I/O; callers run [`parse`] on a background thread.

mod document;
mod highlight;
mod java;
mod kotlin;
mod language;
mod reference;
mod structure;

use std::ops::ControlFlow;

use dumbj_text::{Buffer, ByteOffset, Edit};
use thiserror::Error;
use tree_sitter::{ParseOptions, Parser, Tree};

pub use highlight::{HighlightKind, HighlightSpan};
pub use language::Language;
pub use reference::{
    Argument, Call, LambdaCall, LocalDefinition, Qualifier, Reference, ReferenceRole,
};
pub use structure::{FileFacts, Import, Parameter, Platform, Symbol, SymbolKind};

#[derive(Debug, Error)]
pub enum SyntaxError {
    #[error("the {} grammar is incompatible with this tree-sitter version: {message}", language.name())]
    Grammar { language: Language, message: String },
    #[error("the {} highlight query is invalid: {message}", language.name())]
    Query { language: Language, message: String },
}

#[derive(Debug, Clone)]
pub struct SyntaxTree {
    language: Language,
    tree: Tree,
    buffer_version: u64,
}

impl SyntaxTree {
    #[must_use]
    pub fn language(&self) -> Language {
        self.language
    }

    #[must_use]
    pub fn buffer_version(&self) -> u64 {
        self.buffer_version
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }

    pub(crate) fn tree(&self) -> &Tree {
        &self.tree
    }
}

/// `cancelled` is polled regularly; when it returns `true` parsing stops and `Ok(None)` is
/// returned. Source with syntax errors still yields a tree.
pub fn parse(
    language: Language,
    buffer: &Buffer,
    cancelled: impl Fn() -> bool,
) -> Result<Option<SyntaxTree>, SyntaxError> {
    parse_with(language, buffer, None, cancelled)
}

/// Parses `buffer` again, reusing the parts of `old` that no edit touched. Every edit made
/// to the buffer since `old` was parsed must have been passed to [`SyntaxTree::edit`].
pub fn reparse(
    old: &SyntaxTree,
    buffer: &Buffer,
    cancelled: impl Fn() -> bool,
) -> Result<Option<SyntaxTree>, SyntaxError> {
    parse_with(old.language, buffer, Some(&old.tree), cancelled)
}

fn parse_with(
    language: Language,
    buffer: &Buffer,
    old: Option<&Tree>,
    cancelled: impl Fn() -> bool,
) -> Result<Option<SyntaxTree>, SyntaxError> {
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|error| SyntaxError::Grammar {
            language,
            message: error.to_string(),
        })?;
    let mut progress = |_: &tree_sitter::ParseState| {
        if cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(
        &mut |offset, _| buffer.chunk_at(ByteOffset(offset)).as_bytes(),
        old,
        Some(options),
    );
    Ok(tree.map(|tree| SyntaxTree {
        language,
        tree,
        buffer_version: buffer.version(),
    }))
}

impl SyntaxTree {
    /// Shifts the tree's positions to follow `edit`, so that it stays aligned with the
    /// buffer until [`reparse`] replaces it. The shifted tree is approximate around the edit.
    pub fn edit(&mut self, edit: &Edit) {
        let point = |point: dumbj_text::Point| tree_sitter::Point {
            row: point.line.get(),
            column: point.column,
        };
        self.tree.edit(&tree_sitter::InputEdit {
            start_byte: edit.start.get(),
            old_end_byte: edit.old_end.get(),
            new_end_byte: edit.new_end.get(),
            start_position: point(edit.start_point),
            old_end_position: point(edit.old_end_point),
            new_end_position: point(edit.new_end_point),
        });
    }
}

impl SyntaxTree {
    /// The smallest syntax node strictly larger than `range` that contains it.
    #[must_use]
    pub fn enclosing_range(&self, range: dumbj_text::ByteRange) -> Option<dumbj_text::ByteRange> {
        let mut node = self
            .tree()
            .root_node()
            .descendant_for_byte_range(range.start.get(), range.end.get())?;
        loop {
            let found = ByteOffset(node.start_byte())..ByteOffset(node.end_byte());
            if found.start <= range.start && range.end <= found.end && found != range {
                return Some(found);
            }
            node = node.parent()?;
        }
    }
}

fn is_scope(language: Language, kind: &str) -> bool {
    match language {
        Language::Java => matches!(
            kind,
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "annotation_type_declaration"
                | "method_declaration"
                | "constructor_declaration"
                | "compact_constructor_declaration"
                | "static_initializer"
                | "if_statement"
                | "for_statement"
                | "enhanced_for_statement"
                | "while_statement"
                | "do_statement"
                | "try_statement"
                | "try_with_resources_statement"
                | "switch_expression"
                | "synchronized_statement"
        ),
        Language::Kotlin => matches!(
            kind,
            "class_declaration"
                | "object_declaration"
                | "companion_object"
                | "function_declaration"
                | "secondary_constructor"
                | "anonymous_initializer"
                | "if_expression"
                | "for_statement"
                | "while_statement"
                | "do_while_statement"
                | "when_expression"
                | "try_expression"
                | "lambda_literal"
        ),
        _ => false,
    }
}

impl SyntaxTree {
    /// The first lines of the declarations and blocks that enclose `offset` and start on
    /// an earlier line, outermost first. A header starts after its annotations.
    #[must_use]
    pub fn sticky_scopes(&self, buffer: &Buffer, offset: ByteOffset) -> Vec<dumbj_text::LineIndex> {
        let line = buffer.point_at(offset).line;
        let Some(start) = self
            .tree()
            .root_node()
            .descendant_for_byte_range(offset.get(), offset.get())
        else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut node = Some(start);
        while let Some(current) = node {
            if is_scope(self.language(), current.kind()) && current.end_byte() > offset.get() {
                let mut cursor = current.walk();
                let header = current
                    .children(&mut cursor)
                    .find(|child| {
                        !matches!(
                            child.kind(),
                            "modifiers" | "annotation" | "marker_annotation"
                        )
                    })
                    .unwrap_or(current);
                let header_line = buffer.point_at(ByteOffset(header.start_byte())).line;
                if header_line < line && found.last() != Some(&header_line) {
                    found.push(header_line);
                }
            }
            node = current.parent();
        }
        found.reverse();
        found
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoldKind {
    Code,
    Comment,
    Imports,
}

/// A foldable stretch of lines, `start..=end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// Where the declaration that owns the region starts: Collapse anywhere from here
    /// folds it.
    pub header: dumbj_text::LineIndex,
    pub start: dumbj_text::LineIndex,
    pub end: dumbj_text::LineIndex,
    pub kind: FoldKind,
}

fn fold_kind(language: Language, kind: &str) -> Option<FoldKind> {
    match (language, kind) {
        (_, "block_comment" | "multiline_comment") => Some(FoldKind::Comment),
        (
            Language::Java,
            "class_body"
            | "interface_body"
            | "enum_body"
            | "annotation_type_body"
            | "block"
            | "constructor_body"
            | "switch_block"
            | "array_initializer",
        )
        | (
            Language::Kotlin,
            "class_body"
            | "enum_class_body"
            | "function_body"
            | "control_structure_body"
            | "lambda_literal"
            | "when_expression"
            | "annotated_lambda",
        ) => Some(FoldKind::Code),
        _ => None,
    }
}

impl SyntaxTree {
    /// Foldable regions spanning two lines or more: bodies, blocks, lambdas, block
    /// comments, and the run of imports.
    #[must_use]
    pub fn folds(&self, buffer: &Buffer) -> Vec<Fold> {
        let language = self.language();
        let line = |byte: usize| buffer.point_at(ByteOffset(byte)).line;
        let mut found = Vec::new();
        let mut imports: Option<(dumbj_text::LineIndex, dumbj_text::LineIndex)> = None;
        let mut cursor = self.tree().root_node().walk();
        loop {
            let node = cursor.node();
            let (start, end) = (
                line(node.start_byte()),
                line(node.end_byte().saturating_sub(1)),
            );
            if matches!(node.kind(), "import_declaration" | "import_header") {
                imports = Some(imports.map_or((start, end), |(first, _)| (first, end)));
            } else if let Some(kind) = fold_kind(language, node.kind())
                && end > start
            {
                let owner = node
                    .parent()
                    .filter(|parent| is_scope(language, parent.kind()))
                    .map_or(start, |parent| line(parent.start_byte()));
                found.push(Fold {
                    header: owner.min(start),
                    start,
                    end,
                    kind,
                });
            }
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    if let Some((start, end)) = imports.filter(|(start, end)| end > start) {
                        found.push(Fold {
                            header: start,
                            start,
                            end,
                            kind: FoldKind::Imports,
                        });
                    }
                    return found;
                }
            }
        }
    }
}
