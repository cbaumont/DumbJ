mod blame;
mod changes;
pub mod diff;
mod git;
pub mod graph;
mod log;
mod pull;
mod remote;
mod revision;
mod status;
mod tree;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use blame::BlameLine;
pub use changes::Change;
pub use log::{Commit, LogOptions};
pub use pull::{PullRefs, Remote, pull_base_ref, pull_head_ref};
pub use revision::Branch;
pub use status::FileStatus;
pub use tree::{TreeFile, revision_path, split_revision_path};

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("git: {0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn git_err(e: impl std::fmt::Display) -> VcsError {
    VcsError::Git(e.to_string())
}

fn is_missing(e: &gix::Error) -> bool {
    use gix::discover::upwards::Error;
    matches!(
        e.downcast_any_ref::<Error>(),
        Some(
            Error::NoGitRepository { .. }
                | Error::NoGitRepositoryWithinCeiling { .. }
                | Error::NoGitRepositoryWithinFs { .. }
        )
    )
}

/// A repository with a working tree. Handles are reopened per call, so they stay cheap to
/// clone and always see the current refs.
#[derive(Debug, Clone)]
pub struct Repository {
    workdir: PathBuf,
}

impl Repository {
    /// The repository whose working tree contains `path`; `Ok(None)` when there is none (or
    /// it is bare).
    pub fn discover(path: &Path) -> Result<Option<Self>, VcsError> {
        let repo = match gix::discover(path) {
            Ok(repo) => repo,
            Err(e) if is_missing(&e) => return Ok(None),
            Err(e) => return Err(git_err(e)),
        };
        Ok(repo.workdir().map(|workdir| Self {
            workdir: workdir.to_path_buf(),
        }))
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    fn open(&self) -> Result<gix::Repository, VcsError> {
        gix::open(&self.workdir).map_err(git_err)
    }

    /// The current branch name, or `None` for a detached HEAD.
    pub fn branch(&self) -> Result<Option<String>, VcsError> {
        let repo = self.open()?;
        let name = repo.head_name().map_err(git_err)?;
        Ok(name.map(|n| n.shorten().to_string()))
    }

    /// The hex id of the HEAD commit; `None` in a repository without commits.
    pub fn head_id(&self) -> Result<Option<String>, VcsError> {
        let repo = self.open()?;
        let id = repo.head_id().ok().map(|id| id.to_string());
        Ok(id)
    }

    /// The content of `path` (absolute, or relative to the workdir) in the HEAD commit.
    /// `Ok(None)` when the file is not in HEAD or is not valid UTF-8.
    pub fn head_text(&self, path: &Path) -> Result<Option<String>, VcsError> {
        self.text_at("HEAD", path)
    }

    /// Files that differ from HEAD (staged or not) and untracked files, as paths relative to
    /// the workdir. Ignored files are left out. Polls `cancelled`; `Ok(None)` when cancelled.
    pub fn statuses(
        &self,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<(PathBuf, FileStatus)>>, VcsError> {
        status::run(&self.workdir, cancelled)
    }

    fn relative(&self, path: &Path) -> Option<PathBuf> {
        if path.is_relative() {
            return Some(path.to_path_buf());
        }
        if let Ok(rel) = path.strip_prefix(&self.workdir) {
            return Some(rel.to_path_buf());
        }
        let workdir = self.workdir.canonicalize().ok()?;
        let parent = path.parent()?.canonicalize().ok()?;
        let rel = parent.strip_prefix(&workdir).ok()?;
        Some(rel.join(path.file_name()?))
    }
}

#[cfg(test)]
mod tests;
