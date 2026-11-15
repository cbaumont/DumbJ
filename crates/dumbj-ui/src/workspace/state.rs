use std::path::{Path, PathBuf};

use dumbj_settings::{ProjectState, RecentLocation, state_dir};
use gpui::{AppContext, Context};

use super::Workspace;

/// Library sources are virtual paths inside a jar; they are not the project's files.
fn is_project_file(path: &Path) -> bool {
    dumbj_jars::split_virtual_path(path).is_none()
}

/// Drops entries whose files are gone, so the lists never offer a file that cannot open.
fn forget_missing_files(state: &mut ProjectState) {
    state.recent_files.retain(|path| path.exists());
    state.recent_locations.retain(|place| place.path.exists());
}

impl Workspace {
    /// Finds the project's state directory and reads the state from it in the background.
    pub(super) fn load_project_state(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.state = ProjectState::default();
        self.state_dir = None;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let dir = state_dir(&root)?;
                    let state = ProjectState::load(&dir).unwrap_or_else(|error| {
                        tracing::warn!(%error, "ignoring the unreadable project state");
                        ProjectState::default()
                    });
                    let mut state = state;
                    forget_missing_files(&mut state);
                    Some((dir, state))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some((dir, loaded)) = loaded else { return };
                this.state_dir = Some(dir);
                this.merge_loaded_state(loaded, cx);
            });
        })
        .detach();
    }

    /// Puts the loaded state under what was recorded while it was loading (the files
    /// given on the command line open before the read finishes).
    fn merge_loaded_state(&mut self, loaded: ProjectState, cx: &mut Context<Self>) {
        let early = std::mem::replace(&mut self.state, loaded);
        for path in early.recent_files.iter().rev() {
            self.state.record_file(path);
        }
        for place in early.recent_locations.into_iter().rev() {
            self.state.record_location(place);
        }
        self.save_project_state(cx);
    }

    /// Writes the state in the background. A newer save replaces one that has not started.
    pub(super) fn save_project_state(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.state_dir.clone() else {
            return;
        };
        let state = self.state.clone();
        self.state_save = Some(cx.background_spawn(async move {
            if let Err(error) = state.save(&dir) {
                tracing::warn!(%error, "cannot save the project state");
            }
        }));
    }

    pub(super) fn record_recent_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.project.is_none() || !is_project_file(path) {
            return;
        }
        self.state.record_file(path);
        self.save_project_state(cx);
    }

    /// Remembers where the caret of the active editor is, as a place to come back to.
    pub(super) fn record_current_location(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        if self.project.is_none() || !is_project_file(&tab.path) {
            return;
        }
        let editor = tab.editor.read(cx);
        let Some(text) = editor.caret_line_text() else {
            return;
        };
        let (line, column) = editor.caret_line_column();
        let place = RecentLocation {
            path: tab.path.clone(),
            line,
            column,
            offset: editor.caret_offset().get(),
            text,
        };
        self.state.record_location(place);
        self.save_project_state(cx);
    }
}
