//! Builds an [`Index`] by walking a project directory and parsing its source files in
//! parallel. Honours `.gitignore`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dumbj_syntax::{FileFacts, Language};
use dumbj_text::Buffer;
use ignore::{WalkBuilder, WalkState};

use crate::index::{Index, IndexedFile};

/// Source files larger than this are listed but not parsed.
const MAX_INDEXED_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Reads and parses one file. Failures are logged and leave the file without symbols, so
/// one unreadable file never stops indexing.
fn file_facts(path: &Path, language: Language, cancelled: &AtomicBool) -> FileFacts {
    let too_large = std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_INDEXED_FILE_BYTES);
    if too_large {
        return FileFacts::default();
    }
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot read file for indexing");
            return FileFacts::default();
        }
    };
    let buffer = Buffer::new(&text);
    match dumbj_syntax::parse(language, &buffer, || cancelled.load(Ordering::Relaxed)) {
        Ok(Some(tree)) => tree.facts(&buffer),
        Ok(None) => FileFacts::default(),
        Err(error) => {
            tracing::error!(%error, "cannot parse file for indexing");
            FileFacts::default()
        }
    }
}

/// Builds the index entry for one file under `root`.
#[must_use]
pub fn index_file(root: &Path, path: &Path, cancelled: &AtomicBool) -> IndexedFile {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    // Only languages with navigation support are parsed; YAML has no declarations.
    let language = Language::from_path(path);
    let facts = match language {
        Some(language @ (Language::Java | Language::Kotlin)) => {
            file_facts(path, language, cancelled)
        }
        _ => FileFacts::default(),
    };
    IndexedFile {
        path: path.to_path_buf(),
        relative,
        language,
        facts,
    }
}

/// Indexes every file under `root` that `.gitignore` does not exclude.
///
/// Work is spread over the available cores. `cancelled` is polled between files and during
/// parsing; when it is set the function returns `None`. `progress` is called with the
/// number of files finished so far, from worker threads.
pub fn index_project(
    root: &Path,
    cancelled: &AtomicBool,
    progress: impl Fn(usize) + Sync,
) -> Option<Index> {
    let files = Mutex::new(Vec::<IndexedFile>::new());
    let done = AtomicUsize::new(0);
    // Dotfiles such as `.gitignore` and `.github/` belong in Go to File; only the
    // repository's own `.git` directory is left out.
    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build_parallel();
    walker.run(|| {
        Box::new(|entry| {
            if cancelled.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                return WalkState::Continue;
            }
            let file = index_file(root, entry.path(), cancelled);
            if let Ok(mut files) = files.lock() {
                files.push(file);
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1);
            WalkState::Continue
        })
    });
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let files = files
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Some(Index::new(files))
}

/// The absolute form of `path`, for looking files up in an index built from `root`.
#[must_use]
pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}
