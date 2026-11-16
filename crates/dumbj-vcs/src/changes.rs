use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
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

const PATHS_PER_CALL: usize = 500;

impl Repository {
    /// The git blob id of each of `paths` (relative to the workdir), in order: in `revision`,
    /// or of the working-tree file when `revision` is `None`. `None` for a path that does
    /// not exist there, is not a regular file, or is not valid UTF-8. Nothing is written to
    /// the repository. Polls `cancelled`; `Ok(None)` when cancelled.
    pub fn content_ids(
        &self,
        revision: Option<&str>,
        paths: &[PathBuf],
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<Option<String>>>, VcsError> {
        if let Some(revision) = revision {
            check_revision(revision)?;
        }
        let names: Vec<&str> = paths
            .iter()
            .filter_map(|path| path.to_str())
            .filter(|name| revision.is_some() || self.is_regular_file(Path::new(name)))
            .collect();
        let mut ids: HashMap<&str, String> = HashMap::new();
        for chunk in names.chunks(PATHS_PER_CALL) {
            let found = match revision {
                Some(revision) => self.tree_blobs(revision, chunk, cancelled)?,
                None => self.hash_files(chunk, cancelled)?,
            };
            let Some(found) = found else {
                return Ok(None);
            };
            ids.extend(found);
        }
        Ok(Some(
            paths
                .iter()
                .map(|path| path.to_str().and_then(|name| ids.get(name).cloned()))
                .collect(),
        ))
    }

    fn is_regular_file(&self, relative: &Path) -> bool {
        std::fs::symlink_metadata(self.workdir.join(relative))
            .is_ok_and(|metadata| metadata.is_file())
    }

    fn tree_blobs<'a>(
        &self,
        revision: &str,
        names: &[&'a str],
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<(&'a str, String)>>, VcsError> {
        let mut args = vec!["ls-tree", "-z", revision, "--"];
        args.extend_from_slice(names);
        let Some(raw) = git::run(&self.workdir, &args, cancelled)? else {
            return Ok(None);
        };
        let listed = parse_ls_tree(&raw);
        Ok(Some(
            names
                .iter()
                .filter_map(|&name| {
                    listed
                        .iter()
                        .find(|(path, _)| *path == name)
                        .map(|(_, id)| (name, id.clone()))
                })
                .collect(),
        ))
    }

    fn hash_files<'a>(
        &self,
        names: &[&'a str],
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<(&'a str, String)>>, VcsError> {
        let mut args = vec!["hash-object", "--"];
        args.extend_from_slice(names);
        let Some(raw) = git::run(&self.workdir, &args, cancelled)? else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(&raw);
        Ok(Some(
            names
                .iter()
                .copied()
                .zip(text.lines().map(|line| line.trim().to_owned()))
                .collect(),
        ))
    }
}

/// Parses `git ls-tree -z` output (`<mode> <type> <id>\t<path>\0`), keeping blobs.
fn parse_ls_tree(raw: &[u8]) -> Vec<(&str, String)> {
    raw.split(|&b| b == 0)
        .filter_map(|record| {
            let tab = record.iter().position(|&b| b == b'\t')?;
            let (meta, path) = record.split_at(tab);
            let mut fields = std::str::from_utf8(meta).ok()?.split(' ');
            let (_mode, kind, id) = (fields.next()?, fields.next()?, fields.next()?);
            if kind != "blob" {
                return None;
            }
            Some((std::str::from_utf8(path.get(1..)?).ok()?, id.to_owned()))
        })
        .collect()
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
    fn ls_tree_keeps_blobs_only() {
        let raw = b"100644 blob aaa\ta b.rs\x00040000 tree bbb\tdir\x00160000 commit ccc\tsub\0";
        assert_eq!(parse_ls_tree(raw), [("a b.rs", "aaa".to_owned())]);
        assert_eq!(parse_ls_tree(b""), []);
    }

    #[test]
    fn empty_and_truncated_input() {
        assert_eq!(parse_name_status(b""), []);
        assert_eq!(parse_name_status(b"M\0"), []);
        assert_eq!(parse_name_status(b"R100\0only-old\0"), []);
    }
}
