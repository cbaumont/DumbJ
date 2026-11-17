use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dumbj_syntax::{FileFacts, Language};
use dumbj_text::Buffer;
use ignore::{WalkBuilder, WalkState};

use crate::index::{Index, IndexedFile};
use crate::store::{CacheStamp, IndexStore};

const MAX_INDEXED_FILE_BYTES: u64 = 4 * 1024 * 1024;

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
    text_facts(&text, language, cancelled)
}

pub(crate) fn text_facts(text: &str, language: Language, cancelled: &AtomicBool) -> FileFacts {
    if text.len() as u64 > MAX_INDEXED_FILE_BYTES {
        return FileFacts::default();
    }
    let buffer = Buffer::new(text);
    match dumbj_syntax::parse(language, &buffer, || cancelled.load(Ordering::Relaxed)) {
        Ok(Some(tree)) => tree.facts(&buffer),
        Ok(None) => FileFacts::default(),
        Err(error) => {
            tracing::error!(%error, "cannot parse file for indexing");
            FileFacts::default()
        }
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[must_use]
pub fn index_file(root: &Path, path: &Path, cancelled: &AtomicBool) -> IndexedFile {
    let relative = relative_path(root, path);
    let language = Language::from_path(path);
    let facts = match language {
        Some(language) if language.has_structure() => file_facts(path, language, cancelled),
        _ => FileFacts::default(),
    };
    IndexedFile {
        path: path.to_path_buf(),
        relative,
        language,
        facts,
    }
}

/// Walks `root` in parallel and builds one [`IndexedFile`] per file with `make`, polling
/// `cancelled` between files. Returns `None` when cancelled.
fn walk_files(
    root: &Path,
    cancelled: &AtomicBool,
    progress: impl Fn(usize) + Sync,
    make: impl Fn(&Path) -> IndexedFile + Sync,
) -> Option<Vec<IndexedFile>> {
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
            let file = make(entry.path());
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
    Some(
        files
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// `cancelled` is polled between files and during
/// parsing; when it is set the function returns `None`. `progress` is called with the
/// number of files finished so far, from worker threads.
pub fn index_project(
    root: &Path,
    cancelled: &AtomicBool,
    progress: impl Fn(usize) + Sync,
) -> Option<Index> {
    let files = walk_files(root, cancelled, progress, |path| {
        index_file(root, path, cancelled)
    })?;
    Some(Index::new(files))
}

/// Like [`index_project`], but files whose [`CacheStamp`] matches the one in `store` reuse
/// the stored facts instead of being read and parsed. Afterwards the store is updated with
/// the files that were parsed and the paths that no longer exist. A failure to save is
/// logged and does not affect the result. When cancelled, nothing is saved.
pub fn index_project_cached(
    root: &Path,
    store: &mut IndexStore,
    cancelled: &AtomicBool,
    progress: impl Fn(usize) + Sync,
) -> Option<Index> {
    let stored = store.load().unwrap_or_else(|error| {
        tracing::warn!(%error, "cannot read the index cache; indexing from scratch");
        HashMap::new()
    });
    let changed = Mutex::new(HashMap::<String, CacheStamp>::new());
    let files = walk_files(root, cancelled, progress, |path| {
        if !Language::from_path(path).is_some_and(Language::has_structure) {
            return index_file(root, path, cancelled);
        }
        // Taken before reading, so an edit during parsing is noticed next time.
        let stamp = CacheStamp::of(path);
        let relative = relative_path(root, path);
        if let (Some(stamp), Some((old, file))) = (stamp, stored.get(&relative))
            && *old == stamp
        {
            return IndexedFile {
                path: path.to_path_buf(),
                relative,
                language: file.language,
                facts: file.facts.clone(),
            };
        }
        let file = index_file(root, path, cancelled);
        if let (Some(stamp), Ok(mut changed)) = (stamp, changed.lock()) {
            changed.insert(file.relative.clone(), stamp);
        }
        file
    })?;
    let changed = changed
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed_files: Vec<(&IndexedFile, CacheStamp)> = files
        .iter()
        .filter_map(|file| Some((file, *changed.get(&file.relative)?)))
        .collect();
    let present: HashSet<&str> = files.iter().map(|file| file.relative.as_str()).collect();
    let removed: Vec<String> = stored
        .keys()
        .filter(|path| !present.contains(path.as_str()))
        .cloned()
        .collect();
    if (!changed_files.is_empty() || !removed.is_empty())
        && let Err(error) = store.save(&changed_files, &removed)
    {
        tracing::warn!(%error, "cannot save the index cache");
    }
    Some(Index::new(files))
}

/// Where the cache of the project at `root` lives: `$XDG_CACHE_HOME/dumbj/index/<id>.sqlite`,
/// or under `$HOME/.cache` without it. Never inside the project. `None` when neither
/// variable is set.
#[must_use]
pub fn cache_path(root: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| absolute(root));
    Some(base.join("dumbj").join("index").join(format!(
        "{:016x}.sqlite",
        fnv1a(root.as_os_str().as_encoded_bytes())
    )))
}

/// 64-bit FNV-1a. Unlike `DefaultHasher` it is specified, so cache names stay stable
/// across Rust versions.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[must_use]
pub fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}
