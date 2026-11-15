//! DumbJ's symbol index and dumb navigation engine. No UI; callers run indexing and
//! searches on background threads.

mod fuzzy;
mod index;
mod indexer;
mod project;
mod resolve;
mod search;
mod usages;

pub use fuzzy::{FuzzyMatch, fuzzy_match};
pub use index::{FileId, Index, IndexedFile, SymbolId};
pub use indexer::{absolute, index_file, index_project};
pub use project::{Module, ModuleId, ProjectModel, SourceKind, SourceRoot};
pub use resolve::{Decision, Resolution, Site, Target, Tier, resolve};
pub use search::{HitTarget, SearchHit, SearchScope, file_structure, search};
pub use usages::{Certainty, Usage, UsageTarget, find_usages, usage_target_at};
