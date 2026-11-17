use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lsp_types::notification::DidChangeWatchedFiles;
use lsp_types::{DidChangeWatchedFilesParams, FileChangeType, FileEvent};

use crate::{LspError, Server, path_to_uri};

/// A file's modification time in nanoseconds and its length.
pub(crate) type Stamp = (u128, u64);

/// The files a Java or Kotlin server reads: sources and build files.
#[must_use]
pub fn is_watched(path: &Path) -> bool {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(extension, "java" | "kt" | "kts" | "gradle")
        || matches!(
            name.as_str(),
            "pom.xml" | "gradle.properties" | "libs.versions.toml"
        )
}

#[must_use]
pub fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((modified.as_nanos(), metadata.len()))
}

fn events(
    old: &BTreeMap<PathBuf, Stamp>,
    new: &BTreeMap<PathBuf, Stamp>,
) -> Vec<(PathBuf, FileChangeType)> {
    let mut events = Vec::new();
    for (path, stamp) in new {
        match old.get(path) {
            None => events.push((path.clone(), FileChangeType::CREATED)),
            Some(known) if known != stamp => events.push((path.clone(), FileChangeType::CHANGED)),
            Some(_) => {}
        }
    }
    events.extend(
        old.keys()
            .filter(|path| !new.contains_key(*path))
            .map(|path| (path.clone(), FileChangeType::DELETED)),
    );
    events
}

fn load(record: &Path) -> Option<BTreeMap<PathBuf, Stamp>> {
    let text = fs::read_to_string(record).ok()?;
    Some(
        text.lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, ' ');
                let modified = parts.next()?.parse().ok()?;
                let len = parts.next()?.parse().ok()?;
                Some((PathBuf::from(parts.next()?), (modified, len)))
            })
            .collect(),
    )
}

fn save(record: &Path, files: &BTreeMap<PathBuf, Stamp>) -> std::io::Result<()> {
    let mut text = String::new();
    for (path, (modified, len)) in files {
        if let Some(path) = path.to_str().filter(|path| !path.contains('\n')) {
            let _ = writeln!(text, "{modified} {len} {path}");
        }
    }
    if let Some(dir) = record.parent() {
        fs::create_dir_all(dir)?;
    }
    let temporary = record.with_extension("tmp");
    fs::write(&temporary, text)?;
    fs::rename(temporary, record)
}

/// Tells `server` which of `files` were created, changed or deleted since the list kept in
/// `record`, then keeps `files` there. Without a record the server is assumed to have just
/// read everything, and nothing is sent. Returns how many events were sent.
pub fn sync_files(
    server: &Server,
    record: &Path,
    files: &BTreeMap<PathBuf, Stamp>,
) -> Result<usize, LspError> {
    let sent = match load(record) {
        None => 0,
        Some(known) => {
            let changes: Vec<FileEvent> = events(&known, files)
                .into_iter()
                .map(|(path, typ)| FileEvent::new(path_to_uri(&path), typ))
                .collect();
            if !changes.is_empty() {
                server.notify::<DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
                    changes: changes.clone(),
                })?;
            }
            changes.len()
        }
    };
    save(record, files)?;
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, u128)]) -> BTreeMap<PathBuf, Stamp> {
        entries
            .iter()
            .map(|(path, modified)| (PathBuf::from(path), (*modified, 1)))
            .collect()
    }

    #[test]
    fn creations_changes_and_deletions_are_found() {
        let old = files(&[("/p/A.java", 1), ("/p/B.java", 1), ("/p/C.java", 1)]);
        let new = files(&[("/p/A.java", 1), ("/p/B.java", 2), ("/p/D.java", 1)]);
        let found: Vec<_> = events(&old, &new)
            .into_iter()
            .map(|(path, kind)| (path.to_string_lossy().into_owned(), kind))
            .collect();
        assert_eq!(
            found,
            [
                ("/p/B.java".to_owned(), FileChangeType::CHANGED),
                ("/p/D.java".to_owned(), FileChangeType::CREATED),
                ("/p/C.java".to_owned(), FileChangeType::DELETED),
            ]
        );
    }

    #[test]
    fn the_record_round_trips_paths_with_spaces() {
        let dir = std::env::temp_dir().join(format!("dumbj-watch-{}", std::process::id()));
        let record = dir.join("jdtls.files");
        let list = files(&[("/p/a b/A.java", 7), ("/p/pom.xml", 9)]);
        save(&record, &list).unwrap();
        assert_eq!(load(&record), Some(list));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sources_and_build_files_are_watched() {
        for path in [
            "A.java",
            "b.kt",
            "build.gradle.kts",
            "x/build.gradle",
            "pom.xml",
            "gradle/libs.versions.toml",
        ] {
            assert!(is_watched(Path::new(path)), "{path}");
        }
        for path in ["README.md", "other.xml", "a.json"] {
            assert!(!is_watched(Path::new(path)), "{path}");
        }
    }
}
