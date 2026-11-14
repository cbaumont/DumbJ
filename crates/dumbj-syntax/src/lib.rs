//! DumbJ's tree-sitter layer: language detection, cancellable parsing and highlighting.
//! No UI and no I/O; callers run [`parse`] on a background thread.

mod highlight;
mod language;

use std::ops::ControlFlow;

use dumbj_text::{Buffer, ByteOffset};
use thiserror::Error;
use tree_sitter::{ParseOptions, Parser, Tree};

pub use highlight::{HighlightKind, HighlightSpan};
pub use language::Language;

/// A failure in the syntax layer. These indicate a bug in a bundled grammar or query.
#[derive(Debug, Error)]
pub enum SyntaxError {
    #[error("the {} grammar is incompatible with this tree-sitter version: {message}", language.name())]
    Grammar { language: Language, message: String },
    #[error("the {} highlight query is invalid: {message}", language.name())]
    Query { language: Language, message: String },
}

/// A parsed syntax tree for one version of a buffer.
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

    /// The [`Buffer::version`] this tree was parsed from.
    #[must_use]
    pub fn buffer_version(&self) -> u64 {
        self.buffer_version
    }

    /// Whether the parser had to recover from syntax errors.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }

    pub(crate) fn tree(&self) -> &Tree {
        &self.tree
    }
}

/// Parses `buffer` as `language`.
///
/// `cancelled` is polled regularly; when it returns `true` parsing stops and `Ok(None)` is
/// returned. Source with syntax errors still yields a tree.
pub fn parse(
    language: Language,
    buffer: &Buffer,
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
        None,
        Some(options),
    );
    Ok(tree.map(|tree| SyntaxTree {
        language,
        tree,
        buffer_version: buffer.version(),
    }))
}
