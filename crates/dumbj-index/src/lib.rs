//! DumbJ's symbol index and dumb navigation engine. No UI; callers run indexing and
//! searches on background threads.

mod fuzzy;
mod hierarchy;
mod index;
mod indexer;
mod project;
mod resolve;
mod revision;
mod search;
mod store;
mod usages;

pub use fuzzy::{FuzzyMatch, fuzzy_match};
pub use hierarchy::{
    Caller, GutterMark, callers, direct_subtypes, direct_supertypes, gutter_marks, implementations,
    overriders, super_members, superclass, warm_hierarchy,
};
pub use index::{FileId, FileReader, Index, IndexedFile, SymbolId};
pub use indexer::{absolute, cache_path, index_file, index_project, index_project_cached};
pub use project::{Module, ModuleId, ProjectModel, SourceKind, SourceRoot};
pub use resolve::{
    Decision, LibraryTarget, Resolution, Site, Target, Tier, library_targets, resolve,
};
pub use revision::{RevisionSource, index_revision};
pub use search::{HitTarget, SearchHit, SearchScope, file_structure, search};
pub use store::{CacheStamp, IndexStore, StoreError, StoredFile};
pub use usages::{
    Certainty, Usage, UsageHighlight, UsageTarget, find_usages, usage_highlights, usage_target_at,
};
