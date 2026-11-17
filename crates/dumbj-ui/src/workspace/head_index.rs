use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_index::{FileReader, Index, RevisionSource, index_revision, warm_hierarchy};
use dumbj_syntax::Language;
use dumbj_text::disk::FileStamp;
use dumbj_vcs::{Repository, TreeFile, revision_path, split_revision_path};
use gpui::{AppContext, Context, Task};

use super::Workspace;

const BLOBS_PER_CALL: usize = 256;

pub(super) struct HeadIndex {
    revision: String,
    commit: String,
    index: Option<Arc<Index>>,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl Drop for HeadIndex {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn relative_name(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn has_structure(path: &Path) -> bool {
    Language::from_path(path).is_some_and(Language::has_structure)
}

/// Reads a file of the head: from the working tree while the file there is still the one
/// that was identical to the head when the index was built, from git otherwise.
fn reader(
    repository: Repository,
    commit: String,
    unchanged: HashMap<PathBuf, Option<FileStamp>>,
) -> FileReader {
    FileReader::new(move |path| {
        let (_, at, relative) = split_revision_path(path)?;
        if at != commit {
            return None;
        }
        let on_disk = repository.workdir().join(&relative);
        if let Some(Some(stamp)) = unchanged.get(&relative)
            && FileStamp::of(&on_disk).ok().as_ref() == Some(stamp)
        {
            let bytes = std::fs::read(&on_disk).ok()?;
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        repository.text_at(&commit, &relative).ok().flatten()
    })
}

fn build(
    repository: &Repository,
    commit: &str,
    working: &Index,
    cancelled: &AtomicBool,
) -> Result<Option<Index>, String> {
    let error = |error: dumbj_vcs::VcsError| error.to_string();
    let Some(files) = repository.files_at(commit, cancelled).map_err(error)? else {
        return Ok(None);
    };
    let Some(differing) = repository
        .changed_files(commit, None, cancelled)
        .map_err(error)?
    else {
        return Ok(None);
    };
    let changed: HashSet<PathBuf> = differing
        .into_iter()
        .flat_map(|change| std::iter::once(change.path).chain(change.old_path))
        .collect();
    let workdir = repository.workdir();
    let in_working = |file: &TreeFile| {
        !changed.contains(&file.path) && working.file_by_path(&workdir.join(&file.path)).is_some()
    };
    let to_read: Vec<&TreeFile> = files
        .iter()
        .filter(|file| has_structure(&file.path) && !in_working(file))
        .collect();
    let mut texts: HashMap<&Path, String> = HashMap::new();
    for chunk in to_read.chunks(BLOBS_PER_CALL) {
        let ids: Vec<&str> = chunk.iter().map(|file| file.id.as_str()).collect();
        let Some(blobs) = repository.blobs(&ids, cancelled).map_err(error)? else {
            return Ok(None);
        };
        for (file, blob) in chunk.iter().zip(blobs) {
            let text = blob.map_or_else(String::new, |bytes| {
                String::from_utf8_lossy(&bytes).into_owned()
            });
            texts.insert(&file.path, text);
        }
    }
    let unchanged: HashMap<PathBuf, Option<FileStamp>> = files
        .iter()
        .filter(|file| in_working(file))
        .map(|file| {
            let stamp = FileStamp::of(&workdir.join(&file.path)).ok();
            (file.path.clone(), stamp)
        })
        .collect();
    let sources: Vec<RevisionSource> = files
        .iter()
        .map(|file| RevisionSource {
            path: revision_path(workdir, commit, &file.path),
            relative: relative_name(&file.path),
            text: if in_working(file) {
                None
            } else {
                Some(texts.remove(file.path.as_path()).unwrap_or_default())
            },
        })
        .collect();
    let reader = reader(repository.clone(), commit.to_owned(), unchanged);
    let index = index_revision(working, &sources, reader, cancelled);
    if let Some(index) = &index {
        warm_hierarchy(index);
    }
    Ok(index)
}

impl Workspace {
    pub(super) fn head_path(&self, head: &str, path: &Path) -> Option<PathBuf> {
        let at = self.head_index.as_ref().filter(|at| at.revision == head)?;
        let workdir = self.repository.as_ref()?.workdir();
        let relative = path.strip_prefix(workdir).ok()?;
        Some(revision_path(workdir, &at.commit, relative))
    }

    /// The index that knows `path`: the head's for a file of the review's head commit, none
    /// for a file of any other commit, the working tree's otherwise.
    pub(super) fn index_for(&self, path: &Path) -> Option<Arc<Index>> {
        match split_revision_path(path) {
            Some((_, commit, _)) => self
                .head_index
                .as_ref()
                .filter(|at| at.commit == commit)
                .and_then(|at| at.index.clone()),
            None => self.index.clone(),
        }
    }

    pub(super) fn active_index(&self, cx: &gpui::App) -> Option<Arc<Index>> {
        match self.active_editor() {
            Some(editor) => self.index_for(editor.read(cx).path()),
            None => self.index.clone(),
        }
    }

    pub(super) fn index_head(&mut self, revision: &str, commit: &str, cx: &mut Context<Self>) {
        let same = self
            .head_index
            .as_ref()
            .is_some_and(|at| at.commit == commit && at.revision == revision);
        if !same {
            self.head_index = Some(HeadIndex {
                revision: revision.to_owned(),
                commit: commit.to_owned(),
                index: None,
                cancel: Arc::default(),
                task: None,
            });
        }
        self.build_head_index(cx);
    }

    pub(super) fn drop_head_index(&mut self) {
        self.head_index = None;
    }

    pub(super) fn build_head_index(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(working), Some(at)) = (
            self.repository.clone(),
            self.index.clone(),
            self.head_index.as_mut(),
        ) else {
            return;
        };
        if at.index.is_some() || at.task.is_some() {
            return;
        }
        let (commit, cancel) = (at.commit.clone(), at.cancel.clone());
        at.task = Some(cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let id = commit.clone();
            let built = cx
                .background_spawn(async move { build(&repository, &id, &working, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(at) = this.head_index.as_mut().filter(|at| at.commit == commit) else {
                    return;
                };
                at.task = None;
                match built {
                    Ok(Some(index)) => {
                        tracing::info!(
                            files = index.file_count(),
                            symbols = index.symbol_count(),
                            elapsed = ?started.elapsed(),
                            "review head indexed"
                        );
                        at.index = Some(Arc::new(index));
                        this.index_revision_editors(cx);
                    }
                    Ok(None) => {}
                    Err(message) => {
                        tracing::warn!(%message, "cannot index the review head");
                        this.status_message = Some(message);
                    }
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn head_index_status(&self) -> Option<String> {
        let at = self.head_index.as_ref()?;
        (at.index.is_none() && at.task.is_some()).then(|| "Indexing head…".to_owned())
    }
}
