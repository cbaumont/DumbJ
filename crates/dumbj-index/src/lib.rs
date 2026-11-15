//! DumbJ's symbol index and dumb navigation engine. No UI; callers run indexing and
//! searches on background threads.

mod fuzzy;
mod index;
mod indexer;
mod project;
mod resolve;
mod search;
mod store;
mod usages;

pub use fuzzy::{FuzzyMatch, fuzzy_match};
pub use index::{FileId, Index, IndexedFile, SymbolId};
pub use indexer::{absolute, cache_path, index_file, index_project, index_project_cached};
pub use project::{Module, ModuleId, ProjectModel, SourceKind, SourceRoot};
pub use resolve::{Decision, Resolution, Site, Target, Tier, resolve};
pub use search::{HitTarget, SearchHit, SearchScope, file_structure, search};
pub use store::{FileStamp, IndexStore, StoreError, StoredFile};
pub use usages::{Certainty, Usage, UsageTarget, find_usages, usage_target_at};
