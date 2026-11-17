use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::git;
use crate::revision::check_revision;
use crate::{Repository, VcsError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFile {
    pub path: PathBuf,
    pub id: String,
}

const SEPARATOR: char = '!';
const ID_LENGTH: usize = 40;

/// The path a file of a commit that is not checked out goes by: `<workdir>!<commit>/<path>`.
/// Nothing exists on disk under it.
#[must_use]
pub fn revision_path(workdir: &Path, commit: &str, relative: &Path) -> PathBuf {
    let mut root = workdir.as_os_str().to_owned();
    root.push(format!("{SEPARATOR}{commit}"));
    PathBuf::from(root).join(relative)
}

#[must_use]
pub fn split_revision_path(path: &Path) -> Option<(PathBuf, String, PathBuf)> {
    let text = path.to_str()?;
    let (at, commit) = text.match_indices(SEPARATOR).find_map(|(at, _)| {
        let commit = text.get(at + 1..at + 1 + ID_LENGTH)?;
        let is_id = commit.bytes().all(|b| b.is_ascii_hexdigit());
        let ends = text[at + 1 + ID_LENGTH..].starts_with('/');
        (is_id && ends).then_some((at, commit))
    })?;
    Some((
        PathBuf::from(&text[..at]),
        commit.to_owned(),
        PathBuf::from(&text[at + ID_LENGTH + 2..]),
    ))
}

impl Repository {
    /// Symbolic links and submodules are left out. Polls `cancelled`; `Ok(None)` when cancelled.
    pub fn files_at(
        &self,
        revision: &str,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<TreeFile>>, VcsError> {
        check_revision(revision)?;
        let args = ["ls-tree", "-r", "-z", revision];
        Ok(git::run(&self.workdir, &args, cancelled)?.map(|raw| parse_tree(&raw)))
    }

    /// The contents of the blobs `ids`, in order; `None` for an id that is not a blob of
    /// this repository. Polls `cancelled`; `Ok(None)` when cancelled.
    pub fn blobs(
        &self,
        ids: &[&str],
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<Option<Vec<u8>>>>, VcsError> {
        if ids.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let valid = |id: &&str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit());
        if !ids.iter().all(valid) {
            return Err(VcsError::Git("invalid object id".to_owned()));
        }
        let input = ids.join("\n") + "\n";
        let args = ["cat-file", "--batch"];
        let raw = git::run_input(&self.workdir, &args, input.into_bytes(), cancelled)?;
        Ok(raw.map(|raw| parse_batch(&raw, ids.len())))
    }
}

fn parse_tree(raw: &[u8]) -> Vec<TreeFile> {
    raw.split(|&b| b == 0)
        .filter_map(|record| {
            let tab = record.iter().position(|&b| b == b'\t')?;
            let (meta, path) = record.split_at(tab);
            let mut fields = std::str::from_utf8(meta).ok()?.split(' ');
            let (mode, kind, id) = (fields.next()?, fields.next()?, fields.next()?);
            if kind != "blob" || mode == "120000" {
                return None;
            }
            Some(TreeFile {
                path: PathBuf::from(OsStr::from_bytes(path.get(1..)?)),
                id: id.to_owned(),
            })
        })
        .collect()
}

/// Parses `git cat-file --batch` output: `<id> <type> <size>\n<content>\n` per object, or
/// `<name> missing\n`.
fn parse_batch(raw: &[u8], count: usize) -> Vec<Option<Vec<u8>>> {
    let mut out = Vec::with_capacity(count);
    let mut rest = raw;
    while out.len() < count {
        let Some(end) = rest.iter().position(|&b| b == b'\n') else {
            break;
        };
        let header = String::from_utf8_lossy(&rest[..end]);
        rest = &rest[end + 1..];
        let mut fields = header.split(' ');
        let (_, kind, size) = (fields.next(), fields.next(), fields.next());
        let Some(size) = size.and_then(|size| size.parse::<usize>().ok()) else {
            out.push(None);
            continue;
        };
        let Some(content) = rest.get(..size) else {
            break;
        };
        out.push((kind == Some("blob")).then(|| content.to_vec()));
        rest = rest.get(size + 1..).unwrap_or_default();
    }
    out.resize(count, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_paths_round_trip() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let path = revision_path(Path::new("/work/my!repo"), commit, Path::new("src/A.kt"));
        assert_eq!(
            path,
            PathBuf::from(format!("/work/my!repo!{commit}/src/A.kt"))
        );
        assert_eq!(
            split_revision_path(&path),
            Some((
                PathBuf::from("/work/my!repo"),
                commit.to_owned(),
                PathBuf::from("src/A.kt")
            ))
        );
    }

    #[test]
    fn ordinary_paths_are_not_revision_paths() {
        assert_eq!(split_revision_path(Path::new("/work/repo/src/A.kt")), None);
        assert_eq!(split_revision_path(Path::new("/work/a!b/src/A.kt")), None);
        assert_eq!(
            split_revision_path(Path::new("/lib/x-sources.jar!/pkg/A.kt")),
            None
        );
    }

    #[test]
    fn tree_listing_keeps_regular_files() {
        let raw = b"100644 blob aaa\ta b.rs\x00100755 blob bbb\trun.sh\x00\
            120000 blob ccc\tlink\x00160000 commit ddd\tsub\0";
        let files: Vec<_> = parse_tree(raw)
            .into_iter()
            .map(|file| (file.path, file.id))
            .collect();
        assert_eq!(
            files,
            [
                (PathBuf::from("a b.rs"), "aaa".to_owned()),
                (PathBuf::from("run.sh"), "bbb".to_owned()),
            ]
        );
    }

    #[test]
    fn batch_output_with_missing_and_non_blob_objects() {
        let raw = b"aaa blob 4\none\n\nbbb missing\nccc tree 2\nxx\nddd blob 0\n\n";
        assert_eq!(
            parse_batch(raw, 4),
            [Some(b"one\n".to_vec()), None, None, Some(Vec::new())]
        );
        assert_eq!(parse_batch(b"aaa blob 10\nshort", 2), [None, None]);
    }
}
