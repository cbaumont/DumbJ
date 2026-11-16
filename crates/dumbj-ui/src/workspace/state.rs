use std::path::{Path, PathBuf};

use dumbj_settings::{ProjectState, RecentLocation, state_dir};
use gpui::{AppContext, Context};

use super::Workspace;

/// Whether the project state is being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateSave {
    Idle,
    Running,
    /// The state changed while a save was running, so another save follows it.
    RunningStale,
}

/// Library sources and files of a commit open from virtual paths; they are not the
/// project's files.
pub(super) fn is_project_file(path: &Path) -> bool {
    path.is_absolute()
        && dumbj_jars::split_virtual_path(path).is_none()
        && dumbj_vcs::split_revision_path(path).is_none()
}

/// Drops entries whose files are gone, so the lists never offer a file that cannot open.
fn forget_missing_files(state: &mut ProjectState) {
    state.recent_files.retain(|path| path.exists());
    state.recent_locations.retain(|place| place.path.exists());
    state.bookmarks.retain(|bookmark| bookmark.path.exists());
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
        for bookmark in early.bookmarks {
            if self
                .state
                .bookmark_at(&bookmark.path, bookmark.line)
                .is_none()
            {
                self.state.bookmarks.push(bookmark);
            }
        }
        self.refresh_bookmark_marks(cx);
        self.save_project_state(cx);
        self.smart_state_loaded(cx);
    }

    /// Writes the state in the background, one save at a time so that an older snapshot
    /// can never overwrite a newer one.
    pub(super) fn save_project_state(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.state_dir.clone() else {
            return;
        };
        if self.state_save != StateSave::Idle {
            self.state_save = StateSave::RunningStale;
            return;
        }
        self.state_save = StateSave::Running;
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                if let Err(error) = state.save(&dir) {
                    tracing::warn!(%error, "cannot save the project state");
                }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let changed = this.state_save == StateSave::RunningStale;
                this.state_save = StateSave::Idle;
                if changed {
                    this.save_project_state(cx);
                }
            });
        })
        .detach();
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
        if self.project.is_none() || !is_project_file(&tab.path) || self.in_revision() {
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
