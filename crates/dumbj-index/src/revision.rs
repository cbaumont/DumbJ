use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use dumbj_syntax::{FileFacts, Language};

use crate::index::{FileReader, Index, IndexedFile};
use crate::indexer::text_facts;

#[derive(Debug, Clone)]
pub struct RevisionSource {
    /// Where the file opens from; unique per revision.
    pub path: PathBuf,
    /// Its path in the repository, `/`-separated.
    pub relative: String,
    /// `None` when the file is the same as in the working tree, so the facts of the
    /// working-tree index are reused.
    pub text: Option<String>,
}

/// Returns `None` when cancelled.
pub fn index_revision(
    working: &Index,
    sources: &[RevisionSource],
    reader: FileReader,
    cancelled: &AtomicBool,
) -> Option<Index> {
    let known: HashMap<&str, &IndexedFile> = working
        .files()
        .map(|(_, file)| (file.relative.as_str(), file))
        .collect();
    let next = AtomicUsize::new(0);
    let files = Mutex::new(Vec::with_capacity(sources.len()));
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .min(sources.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !cancelled.load(Ordering::Relaxed) {
                    let Some(source) = sources.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let language = Language::from_path(&source.path);
                    let facts = match (&source.text, language) {
                        (Some(text), Some(language)) if language.has_structure() => {
                            text_facts(text, language, cancelled)
                        }
                        (Some(_), _) => FileFacts::default(),
                        (None, _) => known
                            .get(source.relative.as_str())
                            .map_or_else(FileFacts::default, |file| file.facts.clone()),
                    };
                    let file = IndexedFile {
                        path: source.path.clone(),
                        relative: source.relative.clone(),
                        language,
                        facts,
                    };
                    if let Ok(mut files) = files.lock() {
                        files.push(file);
                    }
                }
            });
        }
    });
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let files = files
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Some(Index::with_reader(files, reader))
}
