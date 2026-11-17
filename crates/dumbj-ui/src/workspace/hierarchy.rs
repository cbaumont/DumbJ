use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dumbj_actions::Action;
use dumbj_index::{
    Caller, Certainty, Index, SymbolId, UsageTarget, callers, direct_subtypes, direct_supertypes,
    gutter_marks, implementations, super_members,
};
use dumbj_settings::UiRole;
use gpui::{
    Context, Entity, IntoElement, MouseButton, ScrollStrategy, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};

mod exact;

use super::Workspace;
use super::navigation::{Chooser, ChooserItem, Location};
use crate::editor::{EditorView, GutterIcon};
use crate::theme;

const ROW_HEIGHT: f32 = 22.;
pub(super) const HIERARCHY_WINDOW_HEIGHT: f32 = 260.;
/// Type hierarchies of widely implemented interfaces can be huge.
const MAX_TYPE_ROWS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HierarchyKind {
    Type,
    Calls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expansion {
    Leaf,
    Collapsed,
    Loading,
    Expanded,
}

#[derive(Debug, Clone)]
struct Row {
    depth: usize,
    label: String,
    detail: String,
    location: Option<Location>,
    symbol: Option<SymbolId>,
    expansion: Expansion,
    emphasised: bool,
    maybe: bool,
    /// The language server's item behind an exact row.
    item: Option<exact::Item>,
}

pub(super) struct HierarchyWindow {
    kind: HierarchyKind,
    /// The index the rows' symbols belong to.
    index: Arc<Index>,
    name: String,
    /// Rows from the language server, whose `server` expands them.
    exact: bool,
    server: Option<dumbj_lsp::Server>,
    direction: exact::Direction,
    rows: Vec<Row>,
    /// The rows of the exact view not shown now.
    stash: Vec<Row>,
    selected: usize,
    scroll: UniformListScrollHandle,
    pub(super) visible: bool,
    pub(super) hidden_by_hide_all: bool,
}

struct Prepared {
    kind: HierarchyKind,
    index: Arc<Index>,
    name: String,
    rows: Vec<Row>,
    selected: usize,
}

impl HierarchyWindow {
    fn title(&self) -> String {
        let view = match (self.kind, self.direction) {
            (HierarchyKind::Type, exact::Direction::Inward) => "Type hierarchy",
            (HierarchyKind::Type, exact::Direction::Outward) => "Supertypes",
            (HierarchyKind::Calls, exact::Direction::Inward) => "Callers",
            (HierarchyKind::Calls, exact::Direction::Outward) => "Callees",
        };
        format!("{view} of {}", self.name)
    }
}

fn symbol_location(index: &Index, id: SymbolId) -> Option<Location> {
    Some(Location {
        path: index.file(id.file)?.path.clone(),
        offset: index.symbol(id)?.name_range.start,
    })
}

fn symbol_detail(index: &Index, id: SymbolId) -> String {
    let file = index.file(id.file);
    let package = file.and_then(|file| file.facts.package.clone());
    let name = file.and_then(|file| {
        file.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });
    [package, name]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn symbol_row(index: &Index, id: SymbolId, depth: usize, expansion: Expansion) -> Row {
    Row {
        depth,
        label: index.qualified_name(id),
        detail: symbol_detail(index, id),
        location: symbol_location(index, id),
        symbol: Some(id),
        expansion,
        emphasised: false,
        maybe: false,
        item: None,
    }
}

fn subtype_rows(
    index: &Index,
    id: SymbolId,
    depth: usize,
    rows: &mut Vec<Row>,
    seen: &mut Vec<SymbolId>,
) {
    for subtype in direct_subtypes(index, id) {
        if rows.len() >= MAX_TYPE_ROWS || seen.contains(&subtype) {
            continue;
        }
        seen.push(subtype);
        let has_children = !direct_subtypes(index, subtype).is_empty();
        let expansion = if has_children {
            Expansion::Expanded
        } else {
            Expansion::Leaf
        };
        rows.push(symbol_row(index, subtype, depth, expansion));
        subtype_rows(index, subtype, depth + 1, rows, seen);
    }
}

/// The supertype chain down to `id`, then every subtype below it.
fn type_rows(index: &Index, id: SymbolId) -> Vec<Row> {
    let mut chain = vec![id];
    while let Some(parent) = chain
        .last()
        .and_then(|last| direct_supertypes(index, *last).into_iter().next())
    {
        if chain.contains(&parent) || chain.len() > 32 {
            break;
        }
        chain.push(parent);
    }
    chain.reverse();
    let mut rows: Vec<Row> = chain
        .iter()
        .enumerate()
        .map(|(depth, ancestor)| symbol_row(index, *ancestor, depth, Expansion::Expanded))
        .collect();
    if let Some(own) = rows.last_mut() {
        own.emphasised = true;
    }
    let mut seen = chain.clone();
    subtype_rows(index, id, chain.len(), &mut rows, &mut seen);
    if let Some(own) = rows.get_mut(chain.len() - 1)
        && direct_subtypes(index, id).is_empty()
    {
        own.expansion = Expansion::Leaf;
    }
    rows
}

fn caller_rows(index: &Index, found: &[Caller], depth: usize) -> Vec<Row> {
    found
        .iter()
        .map(|caller| {
            let first = caller.calls.first();
            let location = first.map(|usage| Location {
                path: usage.path.clone(),
                offset: usage.range.start,
            });
            let maybe = caller
                .calls
                .iter()
                .all(|usage| usage.certainty == Certainty::Maybe);
            let file_name = index
                .file(caller.file)
                .and_then(|file| {
                    file.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            let line = first.map_or(0, |usage| usage.line + 1);
            let count = caller.calls.len();
            let calls = if count == 1 {
                String::new()
            } else {
                format!(" ({count} calls)")
            };
            let callable = caller.symbol.is_some_and(|id| {
                index
                    .symbol(id)
                    .is_some_and(|symbol| symbol.parameters.is_some() && !symbol.kind.is_type())
            });
            Row {
                depth,
                label: caller
                    .symbol
                    .map_or_else(|| file_name.clone(), |id| index.qualified_name(id)),
                detail: format!("{file_name}:{line}{calls}"),
                location,
                symbol: caller.symbol,
                expansion: if callable {
                    Expansion::Collapsed
                } else {
                    Expansion::Leaf
                },
                emphasised: false,
                maybe,
                item: None,
            }
        })
        .collect()
}

/// The index of the row after the subtree of `row`.
fn subtree_end(rows: &[Row], row: usize) -> usize {
    let depth = rows.get(row).map_or(0, |r| r.depth);
    rows.iter()
        .enumerate()
        .skip(row + 1)
        .find(|(_, r)| r.depth <= depth)
        .map_or(rows.len(), |(index, _)| index)
}

fn collapse(rows: &mut Vec<Row>, row: usize) {
    let end = subtree_end(rows, row);
    rows.drain(row + 1..end);
    if let Some(r) = rows.get_mut(row) {
        r.expansion = Expansion::Collapsed;
    }
}

fn insert_children(rows: &mut Vec<Row>, row: usize, children: Vec<Row>) {
    let leaf = children.is_empty();
    if let Some(r) = rows.get_mut(row) {
        r.expansion = if leaf {
            Expansion::Leaf
        } else {
            Expansion::Expanded
        };
    }
    let at = row + 1;
    rows.splice(at..at, children);
}

impl Workspace {
    fn symbol_at_caret(&self, cx: &Context<Self>) -> Option<(Arc<Index>, SymbolId)> {
        let index = self.active_index(cx)?;
        let target = self
            .active_editor()?
            .read(cx)
            .usage_target_at_caret(&index)?;
        match target {
            UsageTarget::Symbol(id) => Some((index, id)),
            UsageTarget::Local { .. } => None,
        }
    }

    fn jump_or_choose(
        &mut self,
        index: &Index,
        title: String,
        targets: &[SymbolId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match targets {
            [] => self.status_message = Some(format!("No {}", title.to_lowercase())),
            [only] => {
                if let Some(location) = symbol_location(index, *only) {
                    self.navigate_to(location, window, cx);
                }
            }
            _ => {
                let items = targets
                    .iter()
                    .filter_map(|id| {
                        Some(ChooserItem::new(
                            index.qualified_name(*id),
                            symbol_detail(index, *id),
                            symbol_location(index, *id)?,
                        ))
                    })
                    .collect();
                self.chooser = Some(Chooser::new(title, items));
                let focus = self.focus.clone();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn goto_implementation_with(
        &mut self,
        smart: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if smart && self.smart_goto(dumbj_lsp::GotoKind::Implementation, window, cx) {
            return;
        }
        let Some((index, id)) = self.symbol_at_caret(cx) else {
            return;
        };
        let name = index
            .symbol(id)
            .map_or_else(String::new, |s| s.name.clone());
        let targets = implementations(&index, id);
        self.jump_or_choose(
            &index,
            format!("Implementations of {name}"),
            &targets,
            window,
            cx,
        );
    }

    pub(super) fn goto_super_method(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, id)) = self.symbol_at_caret(cx) else {
            return;
        };
        let is_type = index.symbol(id).is_some_and(|s| s.kind.is_type());
        let targets = if is_type {
            direct_supertypes(&index, id)
        } else {
            super_members(&index, id)
        };
        let title = if is_type {
            "Supertypes"
        } else {
            "Super methods"
        };
        self.jump_or_choose(&index, title.to_owned(), &targets, window, cx);
    }

    pub(super) fn refresh_gutter_icons(
        &self,
        editor: &Entity<EditorView>,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_for(path) else {
            return;
        };
        editor.update(cx, |editor, cx| editor.set_index(index.clone(), cx));
        let Some(file) = index.file_by_path(path) else {
            return;
        };
        let editor = editor.downgrade();
        cx.spawn(async move |_, cx| {
            let marks_index = index.clone();
            let marks = cx
                .background_spawn(async move { gutter_marks(&marks_index, file) })
                .await;
            let icons = marks
                .iter()
                .filter_map(|mark| {
                    Some(GutterIcon {
                        offset: index.symbol(mark.symbol)?.name_range.start,
                        symbol: mark.symbol,
                        overrides: mark.overrides,
                        overridden: mark.overridden,
                    })
                })
                .collect();
            let _ = editor.update(cx, |editor, cx| editor.set_gutter_icons(icons, cx));
        })
        .detach();
    }

    pub(super) fn gutter_icon_clicked(
        &mut self,
        icon: GutterIcon,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active_index(cx) else {
            return;
        };
        let name = index.qualified_name(icon.symbol);
        let is_type = index.symbol(icon.symbol).is_some_and(|s| s.kind.is_type());
        let mut targets = Vec::new();
        if icon.overrides {
            targets.extend(super_members(&index, icon.symbol));
        }
        if icon.overridden {
            targets.extend(implementations(&index, icon.symbol));
        }
        let title = match (icon.overrides, is_type) {
            (true, _) => format!("Related to {name}"),
            (false, true) => format!("Subtypes of {name}"),
            (false, false) => format!("Implementations of {name}"),
        };
        self.jump_or_choose(&index, title, &targets, window, cx);
    }

    pub(super) fn type_hierarchy(&mut self, cx: &mut Context<Self>) {
        self.hierarchy(HierarchyKind::Type, cx);
    }

    pub(super) fn call_hierarchy(&mut self, cx: &mut Context<Self>) {
        self.hierarchy(HierarchyKind::Calls, cx);
    }

    /// Asks the language server when there is one; the dumb tree is shown at once
    /// without one, or after 300 ms if the exact tree has not arrived.
    fn hierarchy(&mut self, kind: HierarchyKind, cx: &mut Context<Self>) {
        let dumb = match kind {
            HierarchyKind::Type => self.dumb_type_hierarchy(cx),
            HierarchyKind::Calls => self.dumb_call_hierarchy(cx),
        };
        if !self.ask_exact_hierarchy(kind, cx) {
            self.show_dumb_hierarchy(dumb, cx);
            return;
        }
        let generation = self.smart.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::smart_nav::DUMB_AFTER)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.smart.hierarchy_waiting == Some(generation) {
                    this.smart.hierarchy_waiting = None;
                    this.show_dumb_hierarchy(dumb, cx);
                }
            });
        })
        .detach();
    }

    fn show_dumb_hierarchy(
        &mut self,
        dumb: Option<Result<Prepared, String>>,
        cx: &mut Context<Self>,
    ) {
        match dumb {
            None => {}
            Some(Err(message)) => {
                self.status_message = Some(message);
                cx.notify();
            }
            Some(Ok(prepared)) => {
                let kind = prepared.kind;
                self.show_hierarchy(prepared, cx);
                if kind == HierarchyKind::Calls {
                    self.expand_hierarchy_row(0, cx);
                }
            }
        }
    }

    fn dumb_type_hierarchy(&self, cx: &Context<Self>) -> Option<Result<Prepared, String>> {
        let (index, id) = self.symbol_at_caret(cx)?;
        let owner = if index.symbol(id).is_some_and(|s| s.kind.is_type()) {
            Some(id)
        } else {
            std::iter::successors(index.parent(id), |p| index.parent(*p))
                .find(|p| index.symbol(*p).is_some_and(|s| s.kind.is_type()))
        };
        let Some(owner) = owner else {
            return Some(Err("No type at the caret".to_owned()));
        };
        let rows = type_rows(&index, owner);
        let selected = rows.iter().position(|row| row.emphasised).unwrap_or(0);
        Some(Ok(Prepared {
            kind: HierarchyKind::Type,
            name: index.qualified_name(owner),
            index,
            rows,
            selected,
        }))
    }

    fn dumb_call_hierarchy(&self, cx: &Context<Self>) -> Option<Result<Prepared, String>> {
        let (index, id) = self.symbol_at_caret(cx)?;
        let callable = index
            .symbol(id)
            .is_some_and(|s| s.parameters.is_some() && !s.kind.is_type());
        if !callable {
            return Some(Err("No method at the caret".to_owned()));
        }
        let mut root = symbol_row(&index, id, 0, Expansion::Collapsed);
        root.emphasised = true;
        Some(Ok(Prepared {
            kind: HierarchyKind::Calls,
            name: index.qualified_name(id),
            index,
            rows: vec![root],
            selected: 0,
        }))
    }

    fn show_hierarchy(&mut self, prepared: Prepared, cx: &mut Context<Self>) {
        let Prepared {
            kind,
            index,
            name,
            rows,
            selected,
        } = prepared;
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item(selected, ScrollStrategy::Center);
        self.hierarchy_window = Some(HierarchyWindow {
            kind,
            index,
            name,
            exact: false,
            server: None,
            direction: exact::Direction::Inward,
            rows,
            stash: Vec::new(),
            selected,
            scroll,
            visible: true,
            hidden_by_hide_all: false,
        });
        self.pending_focus = Some(super::PendingFocus::Hierarchy);
        cx.notify();
    }

    fn expand_hierarchy_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(hierarchy) = self.hierarchy_window.as_mut() else {
            return;
        };
        let index = hierarchy.index.clone();
        if hierarchy.exact {
            self.expand_exact_row(row, cx);
            return;
        }
        let Some(target) = hierarchy.rows.get_mut(row) else {
            return;
        };
        if hierarchy.kind != HierarchyKind::Calls || target.expansion != Expansion::Collapsed {
            return;
        }
        let Some(id) = target.symbol else { return };
        target.expansion = Expansion::Loading;
        let depth = target.depth + 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let search = index.clone();
            let found = cx
                .background_spawn(async move { callers(&search, id, &AtomicBool::new(false)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(hierarchy) = this.hierarchy_window.as_mut() else {
                    return;
                };
                let still_there = hierarchy
                    .rows
                    .get(row)
                    .is_some_and(|r| r.symbol == Some(id) && r.expansion == Expansion::Loading);
                if still_there {
                    let children = caller_rows(&index, &found.unwrap_or_default(), depth);
                    insert_children(&mut hierarchy.rows, row, children);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn toggle_hierarchy_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.hierarchy_window_focus.is_focused(window);
        let Some(hierarchy) = self.hierarchy_window.as_mut() else {
            return;
        };
        if hierarchy.visible && focused {
            hierarchy.visible = false;
            self.focus_fallback(window, cx);
        } else {
            hierarchy.visible = true;
            let focus = self.hierarchy_window_focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn open_hierarchy_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hierarchy) = self.hierarchy_window.as_mut() else {
            return;
        };
        hierarchy.selected = row;
        if let Some(location) = hierarchy.rows.get(row).and_then(|r| r.location.clone()) {
            self.navigate_to(location, window, cx);
        }
        cx.notify();
    }

    pub(super) fn hierarchy_window_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hierarchy) = self.hierarchy_window.as_mut() else {
            return;
        };
        let last = hierarchy.rows.len().saturating_sub(1);
        let selected = hierarchy.selected;
        let mut select = |row: usize| {
            hierarchy.selected = row.min(last);
            hierarchy
                .scroll
                .scroll_to_item(hierarchy.selected, ScrollStrategy::Nearest);
        };
        match action {
            Action::MoveUp => select(selected.saturating_sub(1)),
            Action::MoveDown => select(selected + 1),
            Action::MovePageUp => select(selected.saturating_sub(8)),
            Action::MovePageDown => select(selected + 8),
            Action::MoveTextStart => select(0),
            Action::MoveTextEnd => select(last),
            Action::MoveRight => self.expand_hierarchy_row(selected, cx),
            Action::MoveLeft => {
                let expanded = hierarchy
                    .rows
                    .get(selected)
                    .is_some_and(|r| r.expansion == Expansion::Expanded);
                if (hierarchy.kind == HierarchyKind::Calls || hierarchy.exact) && expanded {
                    collapse(&mut hierarchy.rows, selected);
                } else if let Some(depth) = hierarchy.rows.get(selected).map(|r| r.depth) {
                    let parent = hierarchy.rows[..selected]
                        .iter()
                        .rposition(|r| r.depth < depth);
                    if let Some(parent) = parent {
                        select(parent);
                    }
                }
            }
            Action::Enter => self.open_hierarchy_row(selected, window, cx),
            Action::Escape => self.focus_fallback(window, cx),
            Action::Tab | Action::Unindent => self.switch_direction(cx),
            _ => {}
        }
        cx.notify();
    }

    fn render_hierarchy_row(
        hierarchy: &HierarchyWindow,
        index: usize,
        row: &Row,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == hierarchy.selected;
        let background = match (selected, focused) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        let marker = match row.expansion {
            Expansion::Leaf => " ",
            Expansion::Collapsed => "▸",
            Expansion::Expanded => "▾",
            Expansion::Loading => "…",
        };
        div()
            .id(("hierarchy-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(12.
                + 16.
                    * f32::from(
                        u16::try_from(row.depth).unwrap_or(u16::MAX),
                    )))
            .pr(px(12.))
            .when_some(background, |r, role| r.bg(theme::ui(cx, role)))
            .when(!selected, |r| {
                r.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let focus = this.hierarchy_window_focus.clone();
                    window.focus(&focus, cx);
                    if event.click_count >= 2 {
                        this.open_hierarchy_row(index, window, cx);
                    } else {
                        if let Some(hierarchy) = this.hierarchy_window.as_mut() {
                            hierarchy.selected = index;
                        }
                        this.expand_hierarchy_row(index, cx);
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .w(px(10.))
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(marker),
            )
            .child(
                div()
                    .flex_none()
                    .when(row.emphasised, |d| d.font_weight(gpui::FontWeight::BOLD))
                    .child(row.label.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(row.detail.clone()),
            )
            .children(row.maybe.then(|| {
                div()
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::Warning))
                    .child("maybe")
            }))
    }

    pub(super) fn render_hierarchy_window(
        &self,
        hierarchy: &HierarchyWindow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.hierarchy_window_focus.is_focused(window);
        let rows = uniform_list(
            "hierarchy-rows",
            hierarchy.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(hierarchy) = this.hierarchy_window.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, hierarchy.rows.get(index)?)))
                    .map(|(index, row)| {
                        Self::render_hierarchy_row(hierarchy, index, row, focused, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&hierarchy.scroll)
        .size_full();
        let title = div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(format!("Hierarchy: {}", hierarchy.title()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .children(Self::render_direction_switch(hierarchy, cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::ui(cx, UiRole::Warning))
                            .child(if hierarchy.exact {
                                "exact"
                            } else {
                                "best guess"
                            }),
                    ),
            );
        div()
            .track_focus(&self.hierarchy_window_focus)
            .h(px(HIERARCHY_WINDOW_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_t_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .child(div().flex_1().min_h_0().child(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(depth: usize, label: &str, expansion: Expansion) -> Row {
        Row {
            depth,
            label: label.to_owned(),
            detail: String::new(),
            location: None,
            symbol: None,
            expansion,
            emphasised: false,
            maybe: false,
            item: None,
        }
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
            .collect()
    }

    #[test]
    fn expanding_and_collapsing_keeps_the_tree_in_order() {
        let mut rows = vec![
            row(0, "root", Expansion::Collapsed),
            row(0, "sibling", Expansion::Leaf),
        ];
        insert_children(
            &mut rows,
            0,
            vec![
                row(1, "a", Expansion::Collapsed),
                row(1, "b", Expansion::Leaf),
            ],
        );
        insert_children(&mut rows, 1, vec![row(2, "a1", Expansion::Leaf)]);
        assert_eq!(labels(&rows), ["root", "  a", "    a1", "  b", "sibling"]);
        assert_eq!(subtree_end(&rows, 0), 4);
        collapse(&mut rows, 0);
        assert_eq!(labels(&rows), ["root", "sibling"]);
        assert_eq!(rows[0].expansion, Expansion::Collapsed);
        insert_children(&mut rows, 1, Vec::new());
        assert_eq!(rows[1].expansion, Expansion::Leaf, "no children: a leaf");
    }
}
