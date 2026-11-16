use std::path::{Path, PathBuf};
use std::sync::Arc;

use dumbj_vcs::Repository;
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
                    Some((repository, branch))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (repository, branch) = found.unzip();
                this.repository = repository;
                this.branch = branch.flatten();
                let open: Vec<(Entity<EditorView>, PathBuf)> = this
                    .tabs
                    .iter()
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
