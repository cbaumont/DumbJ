//! The search popup: Search Everywhere with its All / Classes / Files / Symbols tabs, and
//! File Structure for the current file. Matching runs in the background.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_actions::Action;
use dumbj_index::{FileId, HitTarget, Index, SearchHit, SearchScope, file_structure, search};
use dumbj_settings::UiRole;
use dumbj_syntax::SymbolKind;
use dumbj_text::ByteOffset;
use gpui::{
    Context, IntoElement, MouseButton, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, px, svg, uniform_list,
};

use super::Workspace;
use super::navigation::Location;
use crate::theme;

const ROW_HEIGHT: f32 = 26.;
const MAX_RESULTS: usize = 80;
const MAX_VISIBLE_ROWS: f32 = 14.;

/// What the popup searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchMode {
    /// The whole project, in the given scope.
    Project(SearchScope),
    /// The declarations of one file.
    Structure(FileId),
}

const SCOPES: [(SearchScope, &str); 4] = [
    (SearchScope::All, "All"),
    (SearchScope::Classes, "Classes"),
    (SearchScope::Files, "Files"),
    (SearchScope::Symbols, "Symbols"),
];

/// One result row, ready to draw.
struct Row {
    /// A short kind tag: `class`, `fun`, `file`, …
    kind: &'static str,
    name: String,
    /// Byte offsets of the matched characters in `name`.
    positions: Vec<usize>,
    /// Where the hit lives: container, package or folder.
    detail: String,
    location: Location,
}

pub(super) struct SearchPopup {
    mode: SearchMode,
    rows: Vec<Row>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Stops the running search when the query or scope changes.
    cancel: Arc<AtomicBool>,
}

impl Drop for SearchPopup {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn kind_tag(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::Enum => "enum",
        SymbolKind::Record => "record",
        SymbolKind::Annotation => "annotation",
        SymbolKind::Object | SymbolKind::Companion => "object",
        SymbolKind::TypeAlias => "typealias",
        SymbolKind::Function | SymbolKind::Method => "fun",
        SymbolKind::Constructor => "constructor",
        SymbolKind::Field => "field",
        SymbolKind::Property => "property",
        SymbolKind::EnumConstant => "constant",
    }
}

/// Turns a hit into a row. Runs on the background thread with the search.
fn row(index: &Index, hit: SearchHit, structure: bool) -> Option<Row> {
    match hit.target {
        HitTarget::File(id) => {
            let file = index.file(id)?;
            let (folder, name) = file
                .relative
                .rsplit_once('/')
                .unwrap_or(("", &file.relative));
            // A path query matched against the whole path; show the path as the name then.
            let matched_path = hit.positions.iter().any(|position| *position >= name.len());
            let (name, detail) = if matched_path {
                (file.relative.clone(), String::new())
            } else {
                (name.to_owned(), folder.to_owned())
            };
            let location = Location {
                path: file.path.clone(),
                offset: ByteOffset::ZERO,
            };
            Some(Row {
                kind: "file",
                name,
                positions: hit.positions,
                detail,
                location,
            })
        }
        HitTarget::Symbol(id) => {
            let symbol = index.symbol(id)?;
            let file = index.file(id.file)?;
            let container = index.parent(id).map(|parent| index.qualified_name(parent));
            let detail = if structure {
                // Within one file the container is all that tells rows apart.
                container.unwrap_or_default()
            } else {
                let package = file
                    .facts
                    .package
                    .clone()
                    .filter(|package| !package.is_empty());
                let file_name = file.relative.rsplit('/').next().map(str::to_owned);
                [container, package, file_name]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            Some(Row {
                kind: kind_tag(symbol.kind),
                name: symbol.name.clone(),
                positions: hit.positions,
                detail,
                location: Location {
                    path: file.path.clone(),
                    offset: symbol.name_range.start,
                },
            })
        }
    }
}

fn find_rows(
    index: &Index,
    mode: SearchMode,
    query: &str,
    cancel: &AtomicBool,
) -> Option<Vec<Row>> {
    let (hits, structure) = match mode {
        SearchMode::Project(scope) => (
            search(index, query, scope, MAX_RESULTS, || {
                cancel.load(Ordering::Relaxed)
            })?,
            false,
        ),
        SearchMode::Structure(file) => (file_structure(index, file, query), true),
    };
    Some(
        hits.into_iter()
            .filter_map(|hit| row(index, hit, structure))
            .collect(),
    )
}

impl Workspace {
    /// Opens the search popup in `mode`, or switches an open popup to it.
    pub(super) fn open_search(
        &mut self,
        mode: SearchMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keep_query = self.search.is_some();
        self.search = Some(SearchPopup {
            mode,
            rows: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        if !keep_query {
            self.search_input
                .update(cx, |input, cx| input.set_text("", false, cx));
        }
        let focus = self.search_input.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        self.run_search(cx);
        cx.notify();
    }

    /// Opens File Structure for the active editor's file.
    pub(super) fn open_file_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self
            .index
            .as_ref()
            .zip(self.active_tab())
            .and_then(|(index, tab)| index.file_by_path(&tab.path));
        if let Some(file) = file {
            self.open_search(SearchMode::Structure(file), window, cx);
        }
    }

    /// Starts a background search for the current query; replaces any running one.
    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        let (Some(index), Some(popup)) = (self.index.clone(), self.search.as_mut()) else {
            return;
        };
        popup.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        popup.cancel = cancel.clone();
        let (mode, query) = (popup.mode, self.search_input.read(cx).text().to_owned());
        let task = cx.spawn(async move |this, cx| {
            let flag = cancel.clone();
            let rows = cx
                .background_spawn(async move { find_rows(&index, mode, &query, &flag) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // A newer search owns the popup now; drop this result.
                let Some(popup) = this
                    .search
                    .as_mut()
                    .filter(|_| !cancel.load(Ordering::Relaxed))
                else {
                    return;
                };
                popup.rows = rows.unwrap_or_default();
                popup.selected = 0;
                popup.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        });
        // The cancel flag, not the task handle, stops a superseded search.
        task.detach();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    fn accept_search(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let location = self
            .search
            .take()
            .and_then(|popup| popup.rows.get(row).map(|row| row.location.clone()));
        match location {
            Some(location) => self.navigate_to(location, window, cx),
            None => self.focus_fallback(window, cx),
        }
        cx.notify();
    }

    /// Handles a text action while the search popup is open.
    pub(super) fn search_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(popup) = self.search.as_mut() else {
            return;
        };
        let last = popup.rows.len().saturating_sub(1);
        let select = |popup: &mut SearchPopup, row: usize| {
            popup.selected = row.min(last);
            popup
                .scroll
                .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
        };
        match action {
            Action::MoveUp => select(popup, popup.selected.saturating_sub(1)),
            Action::MoveDown => select(popup, popup.selected + 1),
            Action::MovePageUp => select(popup, popup.selected.saturating_sub(10)),
            Action::MovePageDown => select(popup, popup.selected + 10),
            Action::Enter => {
                let selected = popup.selected;
                self.accept_search(selected, window, cx);
            }
            Action::Escape => self.close_search(window, cx),
            // Tab walks the scopes, as in IDEA.
            Action::Tab => {
                if let SearchMode::Project(scope) = popup.mode {
                    let position = SCOPES.iter().position(|(candidate, _)| *candidate == scope);
                    let next = SCOPES.get((position.unwrap_or(0) + 1) % SCOPES.len());
                    if let Some((next, _)) = next {
                        popup.mode = SearchMode::Project(*next);
                        self.run_search(cx);
                    }
                }
            }
            _ => {
                self.search_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
            }
        }
        cx.notify();
    }

    fn render_search_row(
        popup: &SearchPopup,
        index: usize,
        row: &Row,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == popup.selected;
        // The name split into matched and unmatched runs, matched ones accented.
        let mut runs: Vec<(String, bool)> = Vec::new();
        for (offset, c) in row.name.char_indices() {
            let matched = row.positions.contains(&offset);
            match runs.last_mut() {
                Some((text, was_matched)) if *was_matched == matched => text.push(c),
                _ => runs.push((c.to_string(), matched)),
            }
        }
        let name = div()
            .flex()
            .flex_none()
            .children(runs.into_iter().map(|(text, matched)| {
                let role = if matched {
                    UiRole::Accent
                } else {
                    UiRole::Text
                };
                div().text_color(theme::ui(cx, role)).child(text)
            }));
        div()
            .id(("search-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(10.))
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
                    this.accept_search(index, window, cx);
                }),
            )
            .child(
                div()
                    .w(px(76.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(row.kind),
            )
            .child(name)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(row.detail.clone()),
            )
    }

    /// The scope tabs of a project search, or the title of File Structure.
    fn render_search_header(&self, mode: SearchMode, cx: &mut Context<Self>) -> gpui::Div {
        match mode {
            SearchMode::Project(scope) => {
                div()
                    .flex()
                    .gap(px(4.))
                    .children(SCOPES.iter().map(|(candidate, label)| {
                        let active = *candidate == scope;
                        let role = if active {
                            UiRole::Text
                        } else {
                            UiRole::TextMuted
                        };
                        let candidate = *candidate;
                        div()
                            .id(*label)
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .text_color(theme::ui(cx, role))
                            .when(active, |tab| tab.bg(theme::ui(cx, UiRole::Selected)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_search(SearchMode::Project(candidate), window, cx);
                                }),
                            )
                            .child(*label)
                    }))
            }
            SearchMode::Structure(_) => {
                let file = self
                    .active_tab()
                    .map_or_else(String::new, |tab| tab.title.clone());
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(format!("Structure of {file}"))
            }
        }
    }

    pub(super) fn render_search(
        &self,
        popup: &SearchPopup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let header = self.render_search_header(popup.mode, cx);
        let field = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(4.))
            .bg(theme::ui(cx, UiRole::InputBackground))
            .child(
                svg()
                    .path("icons/search.svg")
                    .size(px(14.))
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextFaint)),
            )
            .child(self.search_input.clone());
        let has_query = !self.search_input.read(cx).text().trim().is_empty();
        let count = popup.rows.len();
        #[expect(clippy::cast_precision_loss, reason = "at most MAX_RESULTS rows")]
        let list_height = px(ROW_HEIGHT * (count as f32).min(MAX_VISIBLE_ROWS));
        let results = if count == 0 {
            let text = if has_query {
                "Nothing found"
            } else {
                "Type to search"
            };
            (has_query || matches!(popup.mode, SearchMode::Project(_))).then(|| {
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(text)
                    .into_any_element()
            })
        } else {
            let rows = uniform_list(
                "search-rows",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let Some(popup) = this.search.as_ref() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|index| Some((index, popup.rows.get(index)?)))
                        .map(|(index, row)| {
                            Self::render_search_row(popup, index, row, cx).into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&popup.scroll)
            .h(list_height);
            Some(rows.into_any_element())
        };
        let panel = div()
            .w(px(680.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_color(theme::ui(cx, UiRole::Text))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(header)
                    // Results come from the tree-sitter index, not a language server.
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::ui(cx, UiRole::TextFaint))
                            .child("dumb index"),
                    ),
            )
            .child(field)
            .children(results);
        div()
            .id("search-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(110.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_search(window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(panel),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dumbj_syntax::{FileFacts, Language, Symbol};
    use dumbj_text::ByteOffset;

    use super::*;

    fn index() -> Index {
        let symbol = |name: &str, kind, parent| Symbol {
            name: name.to_owned(),
            kind,
            range: ByteOffset(0)..ByteOffset(50),
            name_range: ByteOffset(10)..ByteOffset(10 + name.len()),
            parent,
            supertypes: Vec::new(),
            receiver: None,
            type_name: None,
        };
        let facts = FileFacts {
            package: Some("com.shop".to_owned()),
            imports: Vec::new(),
            symbols: vec![
                symbol("Cart", SymbolKind::Class, None),
                symbol("total", SymbolKind::Field, Some(0)),
            ],
        };
        Index::new(vec![dumbj_index::IndexedFile {
            path: PathBuf::from("/p/src/com/shop/Cart.java"),
            relative: "src/com/shop/Cart.java".to_owned(),
            language: Some(Language::Java),
            facts,
        }])
    }

    fn rows(mode: SearchMode, query: &str) -> Vec<String> {
        find_rows(&index(), mode, query, &AtomicBool::new(false))
            .unwrap_or_default()
            .iter()
            .map(|row| {
                format!(
                    "{} {} [{}] @{}",
                    row.kind,
                    row.name,
                    row.detail,
                    row.location.offset.get()
                )
            })
            .collect()
    }

    #[test]
    fn rows_describe_files_and_symbols() {
        assert_eq!(
            rows(SearchMode::Project(SearchScope::All), "cart"),
            [
                "class Cart [com.shop · Cart.java] @10",
                "file Cart.java [src/com/shop] @0"
            ]
        );
        assert_eq!(
            rows(SearchMode::Project(SearchScope::Symbols), "tot"),
            ["field total [Cart · com.shop · Cart.java] @10"]
        );
        assert_eq!(
            rows(SearchMode::Project(SearchScope::Files), "shop/cart"),
            ["file src/com/shop/Cart.java [] @0"],
            "a path query shows the whole path"
        );
        assert_eq!(
            rows(SearchMode::Project(SearchScope::All), ""),
            [] as [String; 0]
        );
    }

    #[test]
    fn structure_rows_show_the_container_only() {
        let file = index().file_by_path(&PathBuf::from("/p/src/com/shop/Cart.java"));
        let Some(file) = file else {
            panic!("fixture file missing")
        };
        assert_eq!(
            rows(SearchMode::Structure(file), ""),
            ["class Cart [] @10", "field total [Cart] @10"]
        );
        assert_eq!(
            rows(SearchMode::Structure(file), "to"),
            ["field total [Cart] @10"]
        );
    }
}
