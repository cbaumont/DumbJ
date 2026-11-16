use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dumbj_vcs::{FileStatus, Repository};
use gpui::{AppContext, Context, Entity};

use super::Workspace;
use super::state::is_project_file;
use crate::editor::EditorView;

impl Workspace {
    /// Finds the project's repository and its branch in the background, then gives every
    /// open editor its file's HEAD text. Also picks up a branch switch made elsewhere.
    pub(super) fn refresh_repository(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let repository = Repository::discover(&root)
                        .inspect_err(|error| tracing::warn!(%error, "cannot open the repository"))
                        .ok()
                        .flatten()?;
                    let branch = repository.branch().ok().flatten();
                    let statuses: HashMap<PathBuf, FileStatus> = repository
                        .statuses(&AtomicBool::new(false))
                        .inspect_err(|error| tracing::warn!(%error, "cannot read the git status"))
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(path, status)| (repository.workdir().join(path), status))
                        .collect();
                    Some((repository, branch, statuses))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (repository, branch, statuses) = match found {
                    Some((repository, branch, statuses)) => (Some(repository), branch, statuses),
                    None => (None, None, HashMap::new()),
                };
                this.repository = repository;
                this.branch = branch;
                if let Some(project) = &this.project {
                    project
                        .tree
                        .update(cx, |tree, cx| tree.set_statuses(statuses, cx));
                }
                let open: Vec<(Entity<EditorView>, PathBuf)> = this
                    .tabs
                    .iter()
                    .filter(|tab| tab.shows_working_file())
                    .map(|tab| (tab.editor.clone(), tab.path.clone()))
                    .collect();
                for (editor, path) in open {
                    this.refresh_base_text(&editor, &path, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn refresh_base_text(
        &self,
        editor: &Entity<EditorView>,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.repository.clone().filter(|_| is_project_file(path)) else {
            editor.update(cx, |editor, cx| editor.set_base_text(None, cx));
            return;
        };
        let (editor, path) = (editor.downgrade(), path.to_path_buf());
        cx.spawn(async move |_, cx| {
            let base = cx
                .background_spawn(async move {
                    repository
                        .head_text(&path)
                        .inspect_err(|error| tracing::warn!(%error, "cannot read the HEAD text"))
                        .ok()
                        .flatten()
                        .map(Arc::new)
                })
                .await;
            let _ = editor.update(cx, |editor, cx| editor.set_base_text(base, cx));
        })
        .detach();
    }
}
