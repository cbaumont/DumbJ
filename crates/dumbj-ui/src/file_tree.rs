use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dumbj_actions::Action;
use dumbj_settings::UiRole;
use dumbj_syntax::Language;
use dumbj_vcs::FileStatus;
use gpui::{
    Context, EventEmitter, FocusHandle, IntoElement, MouseButton, MouseDownEvent, Render,
    ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};

use crate::theme;
use crate::tree_model::{Row, TreeModel, read_dir_sorted};

const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 16.;

pub(crate) struct OpenFile(pub(crate) PathBuf);

/// IDEA's file status colours: blue for modified, green for added, red for unversioned.
pub(crate) fn status_role(status: FileStatus) -> UiRole {
    match status {
        FileStatus::Modified | FileStatus::Renamed => UiRole::Info,
        FileStatus::Added => UiRole::Success,
        FileStatus::Untracked | FileStatus::Conflicted | FileStatus::Deleted => UiRole::Error,
    }
}

pub(crate) struct FileTree {
    root: PathBuf,
    model: TreeModel,
    selected: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    statuses: HashMap<PathBuf, FileStatus>,
}

impl EventEmitter<OpenFile> for FileTree {}

impl FileTree {
    pub(crate) fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let tree = Self {
            root: root.clone(),
            model: TreeModel::default(),
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            statuses: HashMap::new(),
        };
        Self::load(root, cx);
        tree
    }

    /// Sets the git status of the changed files, by absolute path, to colour their names.
    pub(crate) fn set_statuses(
        &mut self,
        statuses: HashMap<PathBuf, FileStatus>,
        cx: &mut Context<Self>,
    ) {
        if self.statuses != statuses {
            self.statuses = statuses;
            cx.notify();
        }
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(crate) fn selected_file(&self) -> Option<&Path> {
        let row = self.model.rows().get(self.selected?)?;
        (!row.entry.is_dir).then_some(row.entry.path.as_path())
    }

    fn load(dir: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let listing_dir = dir.clone();
            let listing = cx
                .background_spawn(async move { read_dir_sorted(&listing_dir) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match listing {
                    Ok(children) if dir == this.root => {
                        this.model.set_root_children(children);
                        this.selected = (!this.model.rows().is_empty()).then_some(0);
                    }
                    Ok(children) => this.model.expand(&dir, children),
                    Err(error) => {
                        tracing::warn!(dir = %dir.display(), %error, "cannot list directory");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let last = self.model.rows().len().saturating_sub(1);
        self.selected = (!self.model.rows().is_empty()).then_some(index.min(last));
        if let Some(index) = self.selected {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.model.rows().get(index) else {
            return;
        };
        if !row.entry.is_dir {
            cx.emit(OpenFile(row.entry.path.clone()));
        } else if row.expanded {
            self.model.collapse(index);
            cx.notify();
        } else {
            Self::load(row.entry.path.clone(), cx);
        }
    }

    pub(crate) fn handle_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.selected else {
            return false;
        };
        let Some(row) = self.model.rows().get(index) else {
            return false;
        };
        let (is_dir, expanded) = (row.entry.is_dir, row.expanded);
        match action {
            Action::MoveUp => self.select(index.saturating_sub(1), cx),
            Action::MoveDown => self.select(index + 1, cx),
            Action::MovePageUp => self.select(index.saturating_sub(20), cx),
            Action::MovePageDown => self.select(index + 20, cx),
            Action::MoveTextStart | Action::MoveLineStart => self.select(0, cx),
            Action::MoveTextEnd | Action::MoveLineEnd => self.select(usize::MAX, cx),
            Action::MoveRight if is_dir && !expanded => self.activate(index, cx),
            Action::MoveRight if is_dir => self.select(index + 1, cx),
            Action::MoveLeft if is_dir && expanded => self.activate(index, cx),
            Action::MoveLeft => {
                if let Some(parent) = self.model.parent_of(index) {
                    self.select(parent, cx);
                }
            }
            Action::Enter => self.activate(index, cx),
            _ => return false,
        }
        true
    }

    fn on_row_mouse_down(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.select(index, cx);
        let Some(row) = self.model.rows().get(index) else {
            return;
        };
        // IDEA: a double click opens a file; a single click is enough for a folder's arrow.
        #[expect(clippy::cast_precision_loss, reason = "tree depth is small")]
        let arrow_right = px(8. + INDENT * (row.depth as f32 + 1.));
        let on_arrow = row.entry.is_dir && event.position.x < arrow_right;
        if event.click_count >= 2 || on_arrow {
            self.activate(index, cx);
        }
    }

    fn render_row(
        &self,
        index: usize,
        row: &Row,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected == Some(index);
        let icon = |path: &'static str, role: UiRole| {
            svg()
                .path(path)
                .size(px(14.))
                .flex_none()
                .text_color(theme::ui(cx, role))
        };
        let arrow = match (row.entry.is_dir, row.expanded) {
            (true, true) => icon("icons/chevron-down.svg", UiRole::TextFaint).into_any_element(),
            (true, false) => icon("icons/chevron-right.svg", UiRole::TextFaint).into_any_element(),
            (false, _) => div().w(px(14.)).flex_none().into_any_element(),
        };
        let kind = if row.entry.is_dir {
            let path = if row.expanded {
                "icons/folder-open.svg"
            } else {
                "icons/folder.svg"
            };
            icon(path, UiRole::FolderIcon)
        } else if Language::from_path(Path::new(&row.entry.name)).is_some() {
            icon("icons/file-code.svg", UiRole::Info)
        } else {
            icon("icons/file.svg", UiRole::FileIcon)
        };
        let background = match (selected, focused) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        #[expect(clippy::cast_precision_loss, reason = "tree depth is small")]
        let indent = px(8. + INDENT * row.depth as f32);
        div()
            .id(index)
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(5.))
            .pl(indent)
            .pr(px(8.))
            .when_some(background, |row, role| row.bg(theme::ui(cx, role)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_row_mouse_down(index, event, window, cx);
                }),
            )
            .child(arrow)
            .child(kind)
            .child(
                div()
                    .truncate()
                    .when_some(self.statuses.get(&row.entry.path), |name, status| {
                        name.text_color(theme::ui(cx, status_role(*status)))
                    })
                    .child(row.entry.name.clone()),
            )
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let rows = uniform_list(
            "project-tree",
            self.model.rows().len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let rows = this.model.rows();
                range
                    .filter_map(|index| Some((index, rows.get(index)?)))
                    .map(|(index, row)| this.render_row(index, row, focused, cx).into_any_element())
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        div()
            .track_focus(&self.focus)
            .size_full()
            .py(px(4.))
            .text_color(theme::ui(cx, UiRole::Text))
            .child(rows)
    }
}
