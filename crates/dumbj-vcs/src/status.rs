use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::{VcsError, git};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Untracked,
    Added,
    Modified,
    Deleted,
    Renamed,
    Conflicted,
}

/// Runs `git status` in `workdir`. The CLI is used because it honours ignore files, the
/// index and rename detection exactly as users expect.
pub(crate) fn run(
    workdir: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<(PathBuf, FileStatus)>>, VcsError> {
    let args = ["status", "--porcelain=v2", "-z", "--untracked-files=all"];
    Ok(git::run(workdir, &args, cancelled)?.map(|out| parse_porcelain_v2(&out)))
}

/// Parses `git status --porcelain=v2 -z` output. Ignored entries, headers and unknown
/// records are skipped.
pub(crate) fn parse_porcelain_v2(raw: &[u8]) -> Vec<(PathBuf, FileStatus)> {
    let mut out = Vec::new();
    let mut records = raw.split(|&b| b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        let path = |bytes: &[u8]| PathBuf::from(OsStr::from_bytes(bytes));
        match record[0] {
            b'?' if record.len() > 2 => out.push((path(&record[2..]), FileStatus::Untracked)),
            b'1' => {
                let fields: Vec<&[u8]> = record.splitn(9, |&b| b == b' ').collect();
                if let [_, xy, .., file] = fields[..] {
                    out.push((path(file), from_xy(xy)));
                }
            }
            b'2' => {
                let fields: Vec<&[u8]> = record.splitn(10, |&b| b == b' ').collect();
                // The original path follows as its own NUL-terminated record.
                records.next();
                if let [.., file] = fields[..]
                    && fields.len() == 10
                {
                    out.push((path(file), FileStatus::Renamed));
                }
            }
            b'u' => {
                let fields: Vec<&[u8]> = record.splitn(11, |&b| b == b' ').collect();
                if let [.., file] = fields[..]
                    && fields.len() == 11
                {
                    out.push((path(file), FileStatus::Conflicted));
                }
            }
            _ => {}
        }
    }
    out
}

fn from_xy(xy: &[u8]) -> FileStatus {
    match xy {
        [b'D', _] | [_, b'D'] => FileStatus::Deleted,
        [b'A', _] | [_, b'A'] => FileStatus::Added,
        _ => FileStatus::Modified,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::FileStatus::*;
    use super::*;
    use crate::tests::TempRepo;

    #[test]
    fn parses_every_record_kind() {
        let raw = b"1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0\
            1 A. N... 000000 100644 100644 000 bbb new.rs\0\
            1 .D N... 100644 100644 000000 aaa bbb gone.rs\0\
            1 D. N... 100644 000000 000000 aaa 000 staged-gone.rs\0\
            1 MM N... 100644 100644 100644 aaa bbb both.rs\0\
            2 R. N... 100644 100644 100644 aaa bbb R100 new name.rs\0old name.rs\0\
            u UU N... 100644 100644 100644 100644 aaa bbb ccc clash.rs\0\
            ? untracked.txt\0\
            ! ignored.txt\0\
            # branch.head main\0";
        let got = parse_porcelain_v2(raw);
        let expected = [
            ("src/a b.rs", Modified),
            ("new.rs", Added),
            ("gone.rs", Deleted),
            ("staged-gone.rs", Deleted),
            ("both.rs", Modified),
            ("new name.rs", Renamed),
            ("clash.rs", Conflicted),
            ("untracked.txt", Untracked),
        ]
        .map(|(p, s)| (PathBuf::from(p), s));
        assert_eq!(got, expected);
    }

    #[test]
    fn empty_and_truncated_input() {
        assert_eq!(parse_porcelain_v2(b""), []);
        assert_eq!(parse_porcelain_v2(b"1 .M\0?\0u UU\0"), []);
    }

    fn sorted(t: &TempRepo) -> Vec<(String, FileStatus)> {
        let mut found: Vec<_> = t
            .repo()
            .statuses(&AtomicBool::new(false))
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|(p, s)| (p.to_string_lossy().into_owned(), s))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    #[test]
    fn reports_changes_against_a_real_repository() {
        let t = TempRepo::new();
        t.file(".gitignore", "*.log\n");
        t.file("keep.txt", "keep\n");
        t.file("edit.txt", "one\n");
        t.file("remove.txt", "bye\n");
        t.file(
            "old.txt",
            "some content that is long enough to be a rename\n",
        );
        t.commit_all();
        assert_eq!(sorted(&t), []);

        t.file("edit.txt", "two\n");
        fs::remove_file(t.0.join("remove.txt")).unwrap();
        t.git(&["mv", "old.txt", "moved.txt"]);
        t.file("staged.txt", "new\n");
        t.git(&["add", "staged.txt"]);
        t.file("dir/untracked.txt", "u\n");
        t.file("noise.log", "ignored\n");

        assert_eq!(
            sorted(&t),
            [
                ("dir/untracked.txt".to_owned(), Untracked),
                ("edit.txt".to_owned(), Modified),
                ("moved.txt".to_owned(), Renamed),
                ("remove.txt".to_owned(), Deleted),
                ("staged.txt".to_owned(), Added),
            ]
        );
    }

    #[test]
    fn a_cancelled_run_returns_none() {
        let t = TempRepo::new();
        assert!(t.repo().statuses(&AtomicBool::new(true)).unwrap().is_none());
    }

    #[test]
    fn statuses_in_a_repository_without_commits() {
        let t = TempRepo::new();
        t.file("a.txt", "a\n");
        assert_eq!(sorted(&t), [("a.txt".to_owned(), Untracked)]);
    }
}
