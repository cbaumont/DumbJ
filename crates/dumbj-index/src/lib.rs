//! DumbJ's symbol index and dumb navigation engine. No UI; callers run indexing and
//! searches on background threads.

mod index;
mod indexer;
mod resolve;

pub use index::{FileId, Index, IndexedFile, SymbolId};
pub use indexer::{absolute, index_file, index_project};
pub use resolve::{Decision, Resolution, Site, Target, Tier, resolve};
