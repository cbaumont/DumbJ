use std::time::Duration;

use dumbj_settings::UiRole;
use gpui::{Context, Entity, IntoElement, Window, div, prelude::*, px};

use super::Workspace;
use crate::editor::{EditorEvent, EditorView};
use crate::theme;

/// Saves that follow each other closely (a tab switch that saves two files) refresh the
/// index once.
const INDEX_REFRESH_DELAY: Duration = Duration::from_millis(300);

impl Workspace {
    pub(super) fn on_editor_event(
        &mut self,
        editor: &Entity<EditorView>,
        event: &EditorEvent,
        cx: &mut Context<Self>,
    ) {
        self.schedule_smart_sync(cx);
        match event {
            EditorEvent::Edited(changes) => {
                let Some(path) = self
                    .tabs
                    .iter()
                    .find(|tab| tab.editor == *editor)
                    .map(|tab| tab.path.clone())
                else {
                    return;
                };
                let mut moved = false;
                for change in changes {
                    moved |=
                        self.state
                            .shift_lines(&path, change.start, change.old_end, change.new_end);
                }
                if moved {
                    self.refresh_bookmark_marks(cx);
                    self.save_project_state(cx);
                }
            }
            EditorEvent::StatusChanged => cx.notify(),
            EditorEvent::Synced => {
                self.schedule_index_refresh(cx);
                self.refresh_repository(cx);
            }
        }
    }

    fn schedule_index_refresh(&mut self, cx: &mut Context<Self>) {
        self.index_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INDEX_REFRESH_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(root) = this.project.as_ref().map(|project| project.root.clone()) {
                    this.start_indexing(root, cx);
                }
            });
        }));
    }

    pub(super) fn save_all(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.editor.update(cx, EditorView::save);
        }
    }

    /// Saves on the calling thread; for quitting, when background work would be cut off.
    pub(super) fn save_all_blocking(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, _| editor.save_blocking());
        }
    }

    pub(super) fn on_window_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            for tab in &self.tabs {
                tab.editor.update(cx, EditorView::check_disk);
            }
            if self.project.is_some() && self.index.is_some() {
                self.schedule_index_refresh(cx);
            }
            self.refresh_repository(cx);
        } else {
            self.save_all(cx);
        }
    }

    /// Shown above an editor whose file changed on disk while it has unsaved edits.
    pub(super) fn render_conflict_bar(
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(theme::ui(cx, UiRole::InputBackground))
                .text_color(theme::ui(cx, UiRole::Text))
                .hover(|style| style.bg(theme::ui(cx, UiRole::Selected)))
                .child(label)
        };
        let (reload, keep) = (editor.clone(), editor.clone());
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .text_size(px(12.))
            .text_color(theme::ui(cx, UiRole::Warning))
            .child("This file changed on disk and has unsaved edits here.")
            .child(
                button("conflict-reload", "Reload from disk").on_click(move |_, _, cx| {
                    reload.update(cx, EditorView::reload_from_disk);
                }),
            )
            .child(
                button("conflict-keep", "Keep my edits").on_click(move |_, _, cx| {
                    keep.update(cx, EditorView::keep_local_changes);
                }),
            )
    }
}
