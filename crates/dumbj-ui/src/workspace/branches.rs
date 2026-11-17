use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dumbj_actions::Action;
use dumbj_settings::UiRole;
use dumbj_vcs::{Branch, Repository};
use gpui::{
    AppContext, Context, Entity, IntoElement, MouseButton, ScrollStrategy, UniformListScrollHandle,
    Window, div, prelude::*, px, uniform_list,
};

use super::Workspace;
use super::git_log::format_time;
use super::popup::{popup_box, popup_layer};
use crate::editor::EditorView;
use crate::theme;

const ROW_HEIGHT: f32 = 26.;
const AUTHOR_CHARS: usize = 15;

#[derive(Clone)]
enum BranchRow {
    Fetch,
    Branch(Branch),
}

impl BranchRow {
    fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetch".to_owned(),
            Self::Branch(branch) => branch.name.clone(),
        }
    }

    fn detail(&self) -> &'static str {
        match self {
            Self::Fetch => "Update the remote branches",
            Self::Branch(branch) if branch.current => "current",
            Self::Branch(branch) if branch.remote => "check out as a local branch",
            Self::Branch(_) => "check out",
        }
    }
}

pub(super) struct BranchPopup {
    branches: Vec<Branch>,
    selected: usize,
    scroll: UniformListScrollHandle,
}

fn rows(branches: &[Branch], query: &str) -> Vec<BranchRow> {
    let needle = query.trim().to_lowercase();
    std::iter::once(BranchRow::Fetch)
        .chain(branches.iter().cloned().map(BranchRow::Branch))
        .filter(|row| row.label().to_lowercase().contains(&needle))
        .collect()
}

/// Git's messages run over several lines; the status bar has room for one.
fn first_line(message: &str) -> String {
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default().to_owned();
    match lines.next() {
        Some(second) if first.ends_with(':') => format!("{first} {second}…"),
        Some(_) => format!("{first}…"),
        None => first,
    }
}

fn list_branches(repository: &Repository) -> Vec<Branch> {
    repository
        .branches()
        .inspect_err(|error| tracing::warn!(%error, "cannot list the branches"))
        .unwrap_or_default()
}

impl Workspace {
    pub(super) fn open_branch_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        self.branch_popup = Some(BranchPopup {
            branches: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
        });
        self.branch_input
            .update(cx, |input, cx| input.set_text("", false, cx));
        let handle = self.branch_input.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.spawn(async move |this, cx| {
            let branches = cx
                .background_spawn(async move { list_branches(&repository) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(popup) = this.branch_popup.as_mut() {
                    popup.branches = branches;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn branch_query_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(popup) = self.branch_popup.as_mut() {
            popup.selected = 0;
            cx.notify();
        }
    }

    fn close_branch_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_popup = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    fn accept_branch_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.branch_popup.take() else {
            return;
        };
        let query = self.branch_input.read(cx).text().to_owned();
        let row = rows(&popup.branches, &query).into_iter().nth(index);
        self.focus_fallback(window, cx);
        match row {
            Some(BranchRow::Fetch) => self.fetch(cx),
            Some(BranchRow::Branch(branch)) if !branch.current => self.checkout(branch, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn branch_popup_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.branch_input.read(cx).text().to_owned();
        let Some(popup) = self.branch_popup.as_mut() else {
            return;
        };
        let last = rows(&popup.branches, &query).len().saturating_sub(1);
        match action {
            Action::MoveUp => popup.selected = popup.selected.saturating_sub(1),
            Action::MoveDown => popup.selected = (popup.selected + 1).min(last),
            Action::MovePageUp => popup.selected = popup.selected.saturating_sub(10),
            Action::MovePageDown => popup.selected = (popup.selected + 10).min(last),
            Action::Enter => {
                let selected = popup.selected;
                self.accept_branch_row(selected, window, cx);
                return;
            }
            Action::Escape => {
                self.close_branch_popup(window, cx);
                return;
            }
            _ => {
                self.branch_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
                return;
            }
        }
        popup
            .scroll
            .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Saves every file, then switches branch in the background and reloads what changed.
    fn checkout(&mut self, branch: Branch, cx: &mut Context<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        // On this thread: git must not start before the last edit is on disk.
        self.save_all_blocking(cx);
        self.status_message = Some(format!("Checking out {}…", branch.name));
        cx.spawn(async move |this, cx| {
            let name = branch.name.clone();
            let result = cx
                .background_spawn(async move { repository.checkout(&branch) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_message = Some(match result {
                    Ok(()) => format!("Checked out {name}"),
                    Err(error) => format!("Checkout failed: {}", first_line(&error.to_string())),
                });
                this.after_git_change(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        self.status_message = Some("Fetching…".to_owned());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { repository.fetch(&AtomicBool::new(false)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_message = Some(match result {
                    Ok(_) => "Fetched".to_owned(),
                    Err(error) => format!("Fetch failed: {}", first_line(&error.to_string())),
                });
                this.after_git_change(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn after_git_change(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.editor.update(cx, EditorView::check_disk);
        }
        self.refresh_repository(cx);
        self.reload_log_if_open(cx);
        if let Some(root) = self.project.as_ref().map(|project| project.root.clone()) {
            self.start_indexing(root, cx);
        }
        cx.notify();
    }

    /// Shows or hides who last changed each line of the active file.
    pub(super) fn toggle_annotate(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab().filter(|tab| tab.shows_working_file()) else {
            return;
        };
        let editor = tab.editor.clone();
        if editor.read(cx).is_annotated() {
            editor.update(cx, |editor, cx| editor.set_annotations(None, cx));
        } else {
            let path = tab.path.clone();
            self.load_annotations(&editor, path, cx);
        }
    }

    /// Reads the blame of `path` at HEAD into `editor`; called again whenever HEAD may
    /// have moved.
    pub(super) fn load_annotations(
        &self,
        editor: &Entity<EditorView>,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        let editor = editor.downgrade();
        cx.spawn(async move |this, cx| {
            let blamed = cx
                .background_spawn(async move {
                    repository.blame("HEAD", &path, &AtomicBool::new(false))
                })
                .await;
            let labels = match blamed {
                Ok(Some(lines)) => lines
                    .iter()
                    .map(|line| {
                        let date: String = format_time(line.time).chars().take(10).collect();
                        let author: String = line.author.chars().take(AUTHOR_CHARS).collect();
                        format!("{date} {author}")
                    })
                    .collect(),
                Ok(None) => return,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.status_message = Some(format!("Annotate failed: {error}"));
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = editor.update(cx, |editor, cx| {
                editor.set_annotations(Some(Arc::new(labels)), cx);
            });
        })
        .detach();
    }

    pub(super) fn render_branch_popup(
        &self,
        popup: &BranchPopup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = rows(&popup.branches, self.branch_input.read(cx).text());
        #[expect(
            clippy::cast_precision_loss,
            reason = "a popup shows at most a dozen rows"
        )]
        let list_height = px(ROW_HEIGHT * rows.len().min(14) as f32);
        let selected = popup.selected;
        let list = uniform_list(
            "branch-rows",
            rows.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| Some((index, rows.get(index)?)))
                    .map(|(index, row)| {
                        Self::render_branch_row(index, index == selected, row, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&popup.scroll)
        .h(list_height);
        let body = popup_box(cx)
            .w(px(560.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .child(
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(Action::GitBranches.title()),
            )
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.branch_input.clone()),
            )
            .child(list);
        popup_layer(
            "branch-popup-layer",
            140.,
            Workspace::close_branch_popup,
            body,
            cx,
        )
    }

    fn render_branch_row(
        index: usize,
        selected: bool,
        row: &BranchRow,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let current = matches!(row, BranchRow::Branch(branch) if branch.current);
        div()
            .id(("branch-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(24.))
            .px(px(10.))
            .rounded(px(4.))
            .when(selected, |row| row.bg(theme::ui(cx, UiRole::Selected)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.accept_branch_row(index, window, cx);
                }),
            )
            .child(
                div()
                    .flex_none()
                    .when(current, |label| {
                        label.text_color(theme::ui(cx, UiRole::Success))
                    })
                    .child(row.label()),
            )
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(row.detail()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, remote: bool, current: bool) -> Branch {
        Branch {
            name: name.to_owned(),
            remote,
            current,
            tip: String::new(),
        }
    }

    #[test]
    fn git_errors_fit_on_one_line() {
        let refused = "git: error: Your local changes would be overwritten by checkout:\n\tInvoice.kt\nPlease commit";
        assert_eq!(
            first_line(refused),
            "git: error: Your local changes would be overwritten by checkout: Invoice.kt…"
        );
        assert_eq!(first_line("no remote\n"), "no remote");
        assert_eq!(first_line("a\nb"), "a…");
    }

    #[test]
    fn the_popup_lists_fetch_then_the_branches_and_filters_both() {
        let branches = [
            branch("main", false, true),
            branch("topic", false, false),
            branch("origin/main", true, false),
        ];
        let labels = |query: &str| -> Vec<String> {
            rows(&branches, query)
                .iter()
                .map(BranchRow::label)
                .collect()
        };
        assert_eq!(labels(""), ["Fetch", "main", "topic", "origin/main"]);
        assert_eq!(labels("MAIN"), ["main", "origin/main"]);
        assert_eq!(labels("fe"), ["Fetch"]);
        let details: Vec<_> = rows(&branches, "").iter().map(BranchRow::detail).collect();
        assert_eq!(
            details,
            [
                "Update the remote branches",
                "current",
                "check out",
                "check out as a local branch"
            ]
        );
    }
}
