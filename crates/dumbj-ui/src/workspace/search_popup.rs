use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_actions::Action;
use dumbj_index::{
    FileId, HitTarget, Index, SearchHit, SearchScope, file_structure, fuzzy_match, search,
};
use dumbj_settings::UiRole;
use dumbj_settings::{ProjectState, RecentLocation};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchMode {
    Project(SearchScope),
    Structure(FileId),
    Recent(RecentKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecentKind {
    Files,
    Locations,
}

const SCOPES: [(SearchScope, &str); 4] = [
    (SearchScope::All, "All"),
    (SearchScope::Classes, "Classes"),
    (SearchScope::Files, "Files"),
    (SearchScope::Symbols, "Symbols"),
];

struct Row {
    kind: &'static str,
    name: String,
    positions: Vec<usize>,
    detail: String,
    location: Location,
}

pub(super) struct SearchPopup {
    mode: SearchMode,
    rows: Vec<Row>,
    selected: usize,
    scroll: UniformListScrollHandle,
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
        SymbolKind::Heading => "heading",
        SymbolKind::Table => "table",
        SymbolKind::Element => "element",
        SymbolKind::Key => "key",
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

/// Which engine the rows come from; recent lists are not index results.
fn source_label(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::Recent(_) => "",
        _ => "dumb index",
    }
}

/// The recent files or places that match `query`, most recent first for an empty query and
/// best match first otherwise.
fn recent_rows(state: &ProjectState, root: &Path, kind: RecentKind, query: &str) -> Vec<Row> {
    let mut scored: Vec<(i32, Row)> = match kind {
        RecentKind::Files => state
            .recent_files
            .iter()
            .filter_map(|path| recent_file_row(state, root, path, query))
            .collect(),
        RecentKind::Locations => state
            .recent_locations
            .iter()
            .filter_map(|place| recent_location_row(place, query))
            .collect(),
    };
    if !query.trim().is_empty() {
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    }
    scored.into_iter().map(|(_, row)| row).collect()
}

fn recent_file_row(
    state: &ProjectState,
    root: &Path,
    path: &Path,
    query: &str,
) -> Option<(i32, Row)> {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    let (folder, name) = relative.rsplit_once('/').unwrap_or(("", &relative));
    let (matched, name, detail) = match fuzzy_match(query, name) {
        Some(found) => (found, name.to_owned(), folder.to_owned()),
        None => (
            fuzzy_match(query, &relative)?,
            relative.clone(),
            String::new(),
        ),
    };
    // Reopen where the caret was last, as IDEA does.
    let offset = state
        .recent_locations
        .iter()
        .find(|place| place.path == path)
        .map_or(ByteOffset::ZERO, |place| ByteOffset(place.offset));
    Some((
        matched.score,
        Row {
            kind: "file",
            name,
            positions: matched.positions,
            detail,
            location: Location {
                path: path.to_path_buf(),
                offset,
            },
        },
    ))
}

fn recent_location_row(place: &RecentLocation, query: &str) -> Option<(i32, Row)> {
    let file = place
        .path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let detail = format!("{file}:{}", place.line);
    let (matched, positions) = match fuzzy_match(query, &place.text) {
        Some(found) => {
            let positions = found.positions.clone();
            (found, positions)
        }
        None => (fuzzy_match(query, &detail)?, Vec::new()),
    };
    Some((
        matched.score,
        Row {
            kind: "",
            name: place.text.clone(),
            positions,
            detail,
            location: Location {
                path: place.path.clone(),
                offset: ByteOffset(place.offset),
            },
        },
    ))
}

fn find_rows(
    index: &Index,
    mode: SearchMode,
    query: &str,
    cancel: &AtomicBool,
) -> Option<Vec<Row>> {
    let (hits, structure) = match mode {
        SearchMode::Recent(_) => return Some(Vec::new()),
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

    pub(super) fn open_project_search(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = match action {
            Action::GotoClass => SearchScope::Classes,
            Action::GotoFile => SearchScope::Files,
            Action::GotoSymbol => SearchScope::Symbols,
            _ => SearchScope::All,
        };
        self.open_search(SearchMode::Project(scope), window, cx);
    }

    pub(super) fn open_file_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self
            .active_index(cx)
            .zip(self.active_editor())
            .and_then(|(index, editor)| index.file_by_path(editor.read(cx).path()));
        if let Some(file) = file {
            self.open_search(SearchMode::Structure(file), window, cx);
        }
    }

    fn run_recent_search(&mut self, kind: RecentKind, cx: &mut Context<Self>) {
        let (Some(project), Some(popup)) = (self.project.as_ref(), self.search.as_mut()) else {
            return;
        };
        let query = self.search_input.read(cx).text().to_owned();
        popup.rows = recent_rows(&self.state, &project.root, kind, &query);
        // The newest file is the one already open, so start on the one before it.
        popup.selected = usize::from(
            kind == RecentKind::Files && query.trim().is_empty() && popup.rows.len() > 1,
        );
        popup
            .scroll
            .scroll_to_item(popup.selected, ScrollStrategy::Top);
        cx.notify();
    }

    pub(super) fn run_search(&mut self, cx: &mut Context<Self>) {
        if let Some(SearchMode::Recent(kind)) = self.search.as_ref().map(|popup| popup.mode) {
            self.run_recent_search(kind, cx);
            return;
        }
        let (Some(index), Some(popup)) = (self.active_index(cx), self.search.as_mut()) else {
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
            .children((!row.kind.is_empty()).then(|| {
                div()
                    .w(px(76.))
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(row.kind)
            }))
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
            SearchMode::Recent(kind) => {
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(match kind {
                        RecentKind::Files => "Recent Files",
                        RecentKind::Locations => "Recent Locations",
                    })
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
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::ui(cx, UiRole::TextFaint))
                            .child(source_label(popup.mode)),
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
            type_argument: None,
            parameters: None,
            platform: None,
            jvm_name: None,
        };
        let facts = FileFacts {
            package: Some("com.shop".to_owned()),
            imports: Vec::new(),
            jvm_name: None,
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

    fn recent_state() -> ProjectState {
        let place = |path: &str, line: usize, offset: usize, text: &str| RecentLocation {
            path: PathBuf::from(path),
            line,
            column: 1,
            offset,
            text: text.to_owned(),
        };
        let mut state = ProjectState::default();
        for path in ["/p/src/Util.java", "/p/src/Cart.java", "/p/README.md"] {
            state.record_file(Path::new(path));
        }
        state.record_location(place("/p/src/Cart.java", 12, 300, "int total = 0;"));
        state.record_location(place("/p/README.md", 1, 0, "# Shop"));
        state
    }

    fn recent(kind: RecentKind, query: &str) -> Vec<String> {
        recent_rows(&recent_state(), Path::new("/p"), kind, query)
            .iter()
            .map(|row| {
                format!(
                    "{} [{}] @{}",
                    row.name,
                    row.detail,
                    row.location.offset.get()
                )
            })
            .collect()
    }

    #[test]
    fn recent_files_list_the_newest_first_and_reopen_at_the_last_caret() {
        assert_eq!(
            recent(RecentKind::Files, ""),
            [
                "README.md [] @0",
                "Cart.java [src] @300",
                "Util.java [src] @0"
            ]
        );
    }

    #[test]
    fn recent_files_filter_by_name_then_by_path() {
        assert_eq!(recent(RecentKind::Files, "cart"), ["Cart.java [src] @300"]);
        assert_eq!(recent(RecentKind::Files, "src/ut"), ["src/Util.java [] @0"]);
        assert_eq!(recent(RecentKind::Files, "zzz"), [] as [String; 0]);
    }

    #[test]
    fn recent_locations_show_the_line_and_filter_by_its_text() {
        assert_eq!(
            recent(RecentKind::Locations, ""),
            [
                "# Shop [README.md:1] @0",
                "int total = 0; [Cart.java:12] @300"
            ]
        );
        assert_eq!(
            recent(RecentKind::Locations, "total"),
            ["int total = 0; [Cart.java:12] @300"]
        );
        assert_eq!(
            recent(RecentKind::Locations, "readme"),
            ["# Shop [README.md:1] @0"],
            "the file name matches too"
        );
    }
}
