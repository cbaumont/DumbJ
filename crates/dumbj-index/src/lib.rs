//! DumbJ's symbol index and dumb navigation engine. No UI; callers run indexing and
//! searches on background threads.

mod index;
mod indexer;

pub use index::{FileId, Index, IndexedFile, SymbolId};
pub use indexer::{absolute, index_file, index_project};
