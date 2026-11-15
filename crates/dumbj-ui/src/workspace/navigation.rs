use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use dumbj_actions::{Action, MouseStroke};
use dumbj_index::{
    Decision, Index, IndexStore, Resolution, Target, cache_path, index_project,
    index_project_cached, warm_hierarchy,
};
use dumbj_settings::UiRole;
use dumbj_text::ByteOffset;
use gpui::{
    Context, IntoElement, MouseButton, ScrollStrategy, Task, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};

use super::Workspace;
use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Location {
    pub(super) path: PathBuf,
    pub(super) offset: ByteOffset,
}

#[derive(Debug, Default)]
pub(super) struct NavigationHistory {
    entries: Vec<Location>,
    current: usize,
}

const MAX_HISTORY: usize = 200;

impl NavigationHistory {
    /// Records a jump from `from` to `to`. Anything that could be reached with Forward is
    /// dropped, as in a browser.
    pub(super) fn record_jump(&mut self, from: Location, to: Location) {
        self.entries.truncate(self.current + 1);
        if self.entries.last() != Some(&from) {
            self.entries.push(from);
        }
        self.entries.push(to);
        let excess = self.entries.len().saturating_sub(MAX_HISTORY);
        self.entries.drain(..excess);
        self.current = self.entries.len() - 1;
    }

    pub(super) fn can_go_back(&self) -> bool {
        self.current > 0
    }

    pub(super) fn can_go_forward(&self) -> bool {
        self.current + 1 < self.entries.len()
    }

    pub(super) fn back(&mut self) -> Option<Location> {
        self.current = self.current.checked_sub(1)?;
        self.entries.get(self.current).cloned()
    }

    pub(super) fn forward(&mut self) -> Option<Location> {
        let next = self.entries.get(self.current + 1)?.clone();
        self.current += 1;
        Some(next)
    }
}

const CHOOSER_ROW_HEIGHT: f32 = 26.;

pub(super) struct ChooserItem {
    label: String,
    detail: String,
    location: Location,
}

impl ChooserItem {
    pub(super) fn new(label: String, detail: String, location: Location) -> Self {
        Self {
            label,
            detail,
            location,
        }
    }
}

pub(super) struct Chooser {
    title: String,
    items: Vec<ChooserItem>,
    selected: usize,
    scroll: UniformListScrollHandle,
}

impl Chooser {
    pub(super) fn new(title: String, items: Vec<ChooserItem>) -> Self {
        Self {
            title,
            items,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

pub(super) struct Indexing {
    done: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Indexing {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Workspace {
    pub(super) fn start_indexing(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let done = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (progress, cancelled) = (done.clone(), cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            let started = std::time::Instant::now();
            let index = cx
                .background_spawn(async move {
                    let report = |count| progress.store(count, Ordering::Relaxed);
                    let store = cache_path(&root)
                        .ok_or_else(|| "no cache directory".to_owned())
                        .and_then(|path| {
                            IndexStore::open(&path).map_err(|error| error.to_string())
                        });
                    let index = match store {
                        Ok(mut store) => {
                            index_project_cached(&root, &mut store, &cancelled, report)
                        }
                        Err(reason) => {
                            tracing::warn!(%reason, "index cache unavailable; indexing without it");
                            index_project(&root, &cancelled, report)
                        }
                    };
                    if let Some(index) = &index {
                        warm_hierarchy(index);
                    }
                    index
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(index) = index {
                    tracing::info!(
                        files = index.file_count(),
                        symbols = index.symbol_count(),
                        elapsed = ?started.elapsed(),
                        "project indexed"
                    );
                    this.index = Some(Arc::new(index));
                    let open: Vec<_> = this
                        .tabs
                        .iter()
                        .map(|tab| (tab.editor.clone(), tab.path.clone()))
                        .collect();
                    for (editor, path) in open {
                        this.refresh_gutter_icons(&editor, &path, cx);
                    }
                }
                this.indexing = None;
                cx.notify();
            });
        });
        self.index = None;
        self.indexing = Some(Indexing {
            done,
            cancel,
            _task: task,
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.indexing.is_some()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn index_status(&self) -> Option<String> {
        match (&self.indexing, &self.index) {
            (Some(indexing), _) => Some(format!(
                "Indexing… {} files",
                indexing.done.load(Ordering::Relaxed)
            )),
            (None, Some(_)) => Some("dumb".to_owned()),
            (None, None) => None,
        }
    }

    fn current_location(&self, cx: &Context<Self>) -> Option<Location> {
        let tab = self.active_tab()?;
        Some(Location {
            path: tab.path.clone(),
            offset: tab.editor.read(cx).caret_offset(),
        })
    }

    /// Opens `location` without touching the history.
    fn show_location(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(location.path, window, cx);
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.go_to_offset(location.offset, cx));
        }
    }

    pub(super) fn navigate_to(
        &mut self,
        to: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(from) = self.current_location(cx) {
            self.history.record_jump(from, to.clone());
        }
        self.show_location(to, window, cx);
    }

    pub(super) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(location) = self.history.back() {
            self.show_location(location, window, cx);
        }
    }

    pub(super) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(location) = self.history.forward() {
            self.show_location(location, window, cx);
        }
    }

    fn chooser_item(index: &Index, target: &Target) -> ChooserItem {
        let file = index
            .file_by_path(&target.path)
            .and_then(|id| index.file(id));
        let file_name = target
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let label = target
            .symbol
            .map_or_else(String::new, |id| index.qualified_name(id));
        let package = file.and_then(|file| file.facts.package.clone());
        let detail = [package, file_name]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let location = Location {
            path: target.path.clone(),
            offset: target.range.start,
        };
        ChooserItem {
            label,
            detail,
            location,
        }
    }

    pub(super) fn goto_declaration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(index), Some(editor)) = (self.index.clone(), self.active_editor()) else {
            return;
        };
        let resolution = editor.read(cx).resolve_at_caret(&index);
        let Some(Resolution {
            reference,
            targets,
            decision,
        }) = resolution
        else {
            return;
        };
        if reference.role == dumbj_syntax::ReferenceRole::Declaration {
            // On a declaration there is nowhere to go, so show where it is used (as IDEA does).
            self.find_usages(true, cx);
            return;
        }
        match (decision, targets.first()) {
            (Decision::Jump, Some(target)) => {
                let to = Location {
                    path: target.path.clone(),
                    offset: target.range.start,
                };
                self.navigate_to(to, window, cx);
            }
            (Decision::Choose, Some(_)) => {
                let items = targets
                    .iter()
                    .map(|target| Self::chooser_item(&index, target))
                    .collect();
                let title = format!("Declarations of {}", reference.name);
                self.chooser = Some(Chooser::new(title, items));
                // The chooser has no text field; keys reach it through the window.
                let focus = self.focus.clone();
                window.focus(&focus, cx);
            }
            _ => {
                let library = editor.read(cx).library_targets_at_caret(&index);
                self.goto_library(reference.name, library, window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn on_editor_mouse(
        &mut self,
        stroke: MouseStroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = self.enabled_actions(window, cx);
        if let Some(action) = self
            .dispatcher
            .mouse_down(stroke, |action| enabled.contains(&action))
        {
            self.perform(action, window, cx);
        }
    }

    fn close_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chooser = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    fn accept_chooser(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let location = self
            .chooser
            .take()
            .and_then(|chooser| chooser.items.into_iter().nth(index))
            .map(|item| item.location);
        match location {
            Some(location) => self.navigate_to(location, window, cx),
            None => self.focus_fallback(window, cx),
        }
        cx.notify();
    }

    pub(super) fn chooser_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chooser) = self.chooser.as_mut() else {
            return;
        };
        let last = chooser.items.len().saturating_sub(1);
        match action {
            Action::MoveUp => chooser.selected = chooser.selected.saturating_sub(1),
            Action::MoveDown => chooser.selected = (chooser.selected + 1).min(last),
            Action::MoveTextStart => chooser.selected = 0,
            Action::MoveTextEnd => chooser.selected = last,
            Action::MovePageUp => chooser.selected = chooser.selected.saturating_sub(10),
            Action::MovePageDown => chooser.selected = (chooser.selected + 10).min(last),
            Action::Enter => {
                let selected = chooser.selected;
                self.accept_chooser(selected, window, cx);
            }
            Action::Escape => self.close_chooser(window, cx),
            _ => {}
        }
        if let Some(chooser) = self.chooser.as_ref() {
            chooser
                .scroll
                .scroll_to_item(chooser.selected, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn render_chooser_row(
        chooser: &Chooser,
        index: usize,
        item: &ChooserItem,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == chooser.selected;
        div()
            .id(("chooser-row", index))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(24.))
            .px(px(10.))
            .w_full()
            .h(px(CHOOSER_ROW_HEIGHT))
            .rounded(px(4.))
            .when(selected, |row| row.bg(theme::ui(cx, UiRole::Selected)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.accept_chooser(index, window, cx);
                }),
            )
            .child(div().flex_none().child(item.label.clone()))
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(item.detail.clone()),
            )
    }

    pub(super) fn render_chooser(chooser: &Chooser, cx: &mut Context<Self>) -> impl IntoElement {
        let count = chooser.items.len();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a popup shows at most a dozen rows"
        )]
        let list_height = px(CHOOSER_ROW_HEIGHT * count.min(14) as f32);
        let rows = uniform_list(
            "chooser-rows",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                let Some(chooser) = this.chooser.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, chooser.items.get(index)?)))
                    .map(|(index, item)| {
                        Self::render_chooser_row(chooser, index, item, cx).into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&chooser.scroll)
        .h(list_height);
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(10.))
            .pb(px(6.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(chooser.title.clone())
            .child(
                div()
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .text_color(theme::ui(cx, UiRole::Warning))
                    .text_size(px(11.))
                    .child("best guess"),
            );
        let popup = div()
            .w(px(720.))
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_color(theme::ui(cx, UiRole::Text))
            .child(header)
            .child(rows);
        div()
            .id("chooser-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_chooser(window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(popup),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, offset: usize) -> Location {
        Location {
            path: PathBuf::from(path),
            offset: ByteOffset(offset),
        }
    }

    #[test]
    fn back_and_forward_walk_the_jumps() {
        let mut history = NavigationHistory::default();
        assert!(!history.can_go_back() && !history.can_go_forward());
        assert_eq!(history.back(), None);
        history.record_jump(at("a", 1), at("b", 2));
        history.record_jump(at("b", 2), at("c", 3));
        assert!(history.can_go_back());
        assert_eq!(history.back(), Some(at("b", 2)));
        assert_eq!(history.back(), Some(at("a", 1)));
        assert_eq!(history.back(), None);
        assert!(history.can_go_forward());
        assert_eq!(history.forward(), Some(at("b", 2)));
        assert_eq!(history.forward(), Some(at("c", 3)));
        assert_eq!(history.forward(), None);
    }

    #[test]
    fn a_jump_after_moving_the_caret_keeps_both_places() {
        let mut history = NavigationHistory::default();
        history.record_jump(at("a", 1), at("b", 2));
        // The user moved within b before jumping again.
        history.record_jump(at("b", 50), at("c", 3));
        assert_eq!(history.back(), Some(at("b", 50)));
        assert_eq!(history.back(), Some(at("b", 2)));
        assert_eq!(history.back(), Some(at("a", 1)));
    }

    #[test]
    fn a_new_jump_drops_the_forward_entries() {
        let mut history = NavigationHistory::default();
        history.record_jump(at("a", 1), at("b", 2));
        history.record_jump(at("b", 2), at("c", 3));
        assert_eq!(history.back(), Some(at("b", 2)));
        history.record_jump(at("b", 2), at("d", 4));
        assert!(!history.can_go_forward());
        assert_eq!(history.back(), Some(at("b", 2)));
        assert_eq!(history.forward(), Some(at("d", 4)));
    }

    #[test]
    fn the_history_is_bounded() {
        let mut history = NavigationHistory::default();
        for step in 0..500 {
            history.record_jump(at("f", step), at("f", step + 1));
        }
        let mut steps = 0;
        while history.back().is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_HISTORY - 1);
    }
}
