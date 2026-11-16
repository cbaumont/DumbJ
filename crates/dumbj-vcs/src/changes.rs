use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::git;
use crate::revision::check_revision;
use crate::{FileStatus, Repository, VcsError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the workdir; the new path for a rename.
    pub path: PathBuf,
    /// The path before a rename.
    pub old_path: Option<PathBuf>,
    /// `Added`, `Modified`, `Deleted`, `Renamed`, or `Untracked` for working-tree files git
    /// does not know yet.
    pub status: FileStatus,
}

impl Repository {
    /// The files that differ between `base` and `head`, sorted by path, with renames
    /// detected. `head = None` compares `base` with the working tree (tracked changes,
    /// staged or not, plus untracked files as `Untracked`). Polls `cancelled`; `Ok(None)`
    /// when cancelled.
    pub fn changed_files(
        &self,
        base: &str,
        head: Option<&str>,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<Change>>, VcsError> {
        check_revision(base)?;
        let mut args = vec!["diff", "--name-status", "-z", "-M", "--no-ext-diff", base];
        if let Some(head) = head {
            check_revision(head)?;
            args.push(head);
        }
        args.push("--");
        let Some(raw) = git::run(&self.workdir, &args, cancelled)? else {
            return Ok(None);
        };
        let mut changes = parse_name_status(&raw);
        if head.is_none() {
            let Some(statuses) = self.statuses(cancelled)? else {
                return Ok(None);
            };
            changes.extend(
                statuses
                    .into_iter()
                    .filter(|(_, status)| *status == FileStatus::Untracked)
                    .map(|(path, status)| Change {
                        path,
                        old_path: None,
                        status,
                    }),
            );
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Some(changes))
    }
}

/// Parses `git diff --name-status -z` output: a status letter record (with a similarity
/// score for renames) followed by one path record, or two for renames and copies.
pub(crate) fn parse_name_status(raw: &[u8]) -> Vec<Change> {
    let path = |bytes: &[u8]| PathBuf::from(OsStr::from_bytes(bytes));
    let mut out = Vec::new();
    let mut records = raw.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(code) = records.next() {
        match code[0] {
            b'R' | b'C' => {
                let (Some(old), Some(new)) = (records.next(), records.next()) else {
                    break;
                };
                let renamed = code[0] == b'R';
                out.push(Change {
                    path: path(new),
                    old_path: renamed.then(|| path(old)),
                    status: if renamed {
                        FileStatus::Renamed
                    } else {
                        FileStatus::Added
                    },
                });
            }
            letter => {
                let Some(file) = records.next() else {
                    break;
                };
                let status = match letter {
                    b'A' => FileStatus::Added,
                    b'D' => FileStatus::Deleted,
                    _ => FileStatus::Modified,
                };
                out.push(Change {
                    path: path(file),
                    old_path: None,
                    status,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::FileStatus::*;
    use super::*;

    fn c(path: &str, old: Option<&str>, status: FileStatus) -> Change {
        Change {
            path: path.into(),
            old_path: old.map(Into::into),
            status,
        }
    }

    #[test]
    fn parses_every_status_letter() {
        let raw = b"A\0new file.rs\0M\0m.rs\0D\0gone.rs\0T\0typed\0\
            R095\0old name.rs\0new name.rs\0C100\0src\0copy\0U\0clash\0";
        assert_eq!(
            parse_name_status(raw),
            [
                c("new file.rs", None, Added),
                c("m.rs", None, Modified),
                c("gone.rs", None, Deleted),
                c("typed", None, Modified),
                c("new name.rs", Some("old name.rs"), Renamed),
                c("copy", None, Added),
                c("clash", None, Modified),
            ]
        );
    }

    #[test]
    fn empty_and_truncated_input() {
        assert_eq!(parse_name_status(b""), []);
        assert_eq!(parse_name_status(b"M\0"), []);
        assert_eq!(parse_name_status(b"R100\0only-old\0"), []);
    }
}
