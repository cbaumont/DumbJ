use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dumbj_lsp::TypeItem;
use dumbj_lsp::{
    Server, call_hierarchy_prepare, from_lsp, incoming_calls, outgoing_calls, subtypes, supertypes,
    type_hierarchy_prepare, uri_to_path,
};
use dumbj_settings::UiRole;
use dumbj_text::Buffer;
use gpui::{AppContext, Context, IntoElement, MouseButton, div, prelude::*, px};
use lsp_types::{CallHierarchyItem, Position, Uri};

use super::super::Workspace;
use super::super::navigation::Location;
use super::{Expansion, HierarchyKind, HierarchyWindow, Prepared, Row, insert_children};
use crate::theme;

const GIVE_UP_AFTER: Duration = Duration::from_secs(10);
const MAX_SUPERTYPES: usize = 32;

#[derive(Debug, Clone)]
pub(super) enum Item {
    Type(TypeItem),
    Call(CallHierarchyItem),
}

impl Item {
    fn parts(&self) -> (&Uri, &str, Option<&str>, Position) {
        match self {
            Self::Type(item) => (
                &item.uri,
                &item.name,
                item.detail.as_deref(),
                item.selection_range.start,
            ),
            Self::Call(item) => (
                &item.uri,
                &item.name,
                item.detail.as_deref(),
                item.selection_range.start,
            ),
        }
    }

    fn same(&self, other: &Self) -> bool {
        let (uri, name, _, start) = self.parts();
        let (other_uri, other_name, _, other_start) = other.parts();
        uri == other_uri && name == other_name && start == other_start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Callers,
    Callees,
}

/// File texts read while building rows, so several rows in one file read it once.
#[derive(Default)]
struct Texts(HashMap<PathBuf, Option<Buffer>>);

impl Texts {
    fn get(&mut self, path: &Path) -> Option<&Buffer> {
        self.0
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|text| Buffer::new(&text))
            })
            .as_ref()
    }
}

fn row(item: Item, depth: usize, expansion: Expansion, texts: &mut Texts) -> Row {
    let (uri, name, detail, start) = item.parts();
    let path = uri_to_path(uri);
    let location = path.as_ref().and_then(|path| {
        let buffer = texts.get(path)?;
        Some(Location {
            path: path.clone(),
            offset: from_lsp(buffer, start),
        })
    });
    let file = path
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            let address = uri.as_str().split('?').next().unwrap_or("");
            address.rsplit('/').next().map(str::to_owned)
        })
        .unwrap_or_default();
    let place = format!("{file}:{}", start.line + 1);
    Row {
        depth,
        label: name.to_owned(),
        detail: [
            detail.filter(|detail| !detail.is_empty()),
            Some(place.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
        location,
        symbol: None,
        expansion,
        emphasised: false,
        maybe: false,
        item: Some(item),
    }
}

/// The supertype chain down to the type at the caret, which is emphasised and
/// expanded once shown.
fn type_tree(
    server: &Server,
    path: &Path,
    position: Position,
) -> Option<(String, Vec<Row>, usize)> {
    let root = type_hierarchy_prepare(server, path, position, GIVE_UP_AFTER)
        .ok()?
        .into_iter()
        .next()?;
    uri_to_path(&root.uri)?;
    let mut chain = vec![root];
    while chain.len() <= MAX_SUPERTYPES {
        let Some(last) = chain.last().cloned() else {
            break;
        };
        let parent = supertypes(server, last, GIVE_UP_AFTER)
            .ok()
            .and_then(|found| found.into_iter().next());
        match parent {
            Some(parent)
                if !chain
                    .iter()
                    .any(|seen| seen.uri == parent.uri && seen.name == parent.name) =>
            {
                chain.push(parent);
            }
            _ => break,
        }
    }
    let name = chain.first().map(|root| root.name.clone())?;
    chain.reverse();
    let own = chain.len() - 1;
    let mut texts = Texts::default();
    let rows = chain
        .into_iter()
        .enumerate()
        .map(|(depth, item)| {
            let mut row = row(Item::Type(item), depth, Expansion::Expanded, &mut texts);
            if depth == own {
                row.emphasised = true;
                row.expansion = Expansion::Collapsed;
            }
            row
        })
        .collect();
    Some((name, rows, own))
}

fn call_tree(
    server: &Server,
    path: &Path,
    position: Position,
) -> Option<(String, Vec<Row>, usize)> {
    let root = call_hierarchy_prepare(server, path, position, GIVE_UP_AFTER)
        .ok()?
        .into_iter()
        .next()?;
    uri_to_path(&root.uri)?;
    let name = root.name.clone();
    let mut row = row(
        Item::Call(root),
        0,
        Expansion::Collapsed,
        &mut Texts::default(),
    );
    row.emphasised = true;
    Some((name, vec![row], 0))
}

fn children(server: &Server, item: Item, direction: Direction, depth: usize) -> Vec<Row> {
    let items: Vec<Item> = match item {
        Item::Type(item) => subtypes(server, item, GIVE_UP_AFTER)
            .unwrap_or_default()
            .into_iter()
            .map(Item::Type)
            .collect(),
        Item::Call(item) => match direction {
            Direction::Callers => incoming_calls(server, item, GIVE_UP_AFTER),
            Direction::Callees => outgoing_calls(server, item, GIVE_UP_AFTER),
        }
        .unwrap_or_default()
        .into_iter()
        .map(Item::Call)
        .collect(),
    };
    let mut texts = Texts::default();
    items
        .into_iter()
        .map(|item| row(item, depth, Expansion::Collapsed, &mut texts))
        .collect()
}

/// Keeps only the root and folds it, so it can be expanded the other way.
fn reset_to_root(rows: &mut Vec<Row>) {
    rows.truncate(1);
    if let Some(root) = rows.first_mut() {
        root.expansion = Expansion::Collapsed;
    }
}

impl Workspace {
    /// Asks the language server of the active file for the hierarchy at the caret.
    /// Returns false when there is no server to ask.
    pub(super) fn ask_exact_hierarchy(
        &mut self,
        kind: HierarchyKind,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(origin) = self.current_location(cx) else {
            return false;
        };
        let (Some(server), Some(index), Some(editor)) = (
            self.smart_server(&origin.path),
            self.active_index(cx),
            self.active_editor(),
        ) else {
            return false;
        };
        let Some(position) = editor.read(cx).lsp_caret() else {
            return false;
        };
        self.smart.generation += 1;
        let generation = self.smart.generation;
        self.smart.hierarchy_waiting = Some(generation);
        let asked = server.clone();
        let tree = cx.background_spawn(async move {
            match kind {
                HierarchyKind::Type => type_tree(&asked, &origin.path, position),
                HierarchyKind::Calls => call_tree(&asked, &origin.path, position),
            }
        });
        cx.spawn(async move |this, cx| {
            let Some((name, rows, selected)) = tree.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.smart.generation != generation {
                    return;
                }
                this.smart.hierarchy_waiting = None;
                let prepared = Prepared {
                    kind,
                    index,
                    name,
                    rows,
                    selected,
                };
                this.show_hierarchy(prepared, cx);
                if let Some(hierarchy) = this.hierarchy_window.as_mut() {
                    hierarchy.exact = true;
                    hierarchy.server = Some(server);
                }
                this.expand_hierarchy_row(selected, cx);
            });
        })
        .detach();
        true
    }

    pub(super) fn expand_exact_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(hierarchy) = self.hierarchy_window.as_mut() else {
            return;
        };
        let (Some(server), direction) = (hierarchy.server.clone(), hierarchy.direction) else {
            return;
        };
        let Some(target) = hierarchy
            .rows
            .get_mut(row)
            .filter(|target| target.expansion == Expansion::Collapsed)
        else {
            return;
        };
        let Some(item) = target.item.clone() else {
            return;
        };
        target.expansion = Expansion::Loading;
        let depth = target.depth + 1;
        cx.notify();
        let asked = item.clone();
        let found = cx.background_spawn(async move { children(&server, asked, direction, depth) });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |this, cx| {
                let Some(hierarchy) = this.hierarchy_window.as_mut() else {
                    return;
                };
                let still_there = hierarchy.direction == direction
                    && hierarchy.rows.get(row).is_some_and(|r| {
                        r.expansion == Expansion::Loading
                            && r.item.as_ref().is_some_and(|shown| shown.same(&item))
                    });
                if still_there {
                    insert_children(&mut hierarchy.rows, row, found);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows the callees of an exact call hierarchy instead of its callers, or back.
    pub(super) fn switch_call_direction(&mut self, cx: &mut Context<Self>) {
        let Some(hierarchy) = self
            .hierarchy_window
            .as_mut()
            .filter(|hierarchy| hierarchy.exact && hierarchy.kind == HierarchyKind::Calls)
        else {
            return;
        };
        hierarchy.direction = match hierarchy.direction {
            Direction::Callers => Direction::Callees,
            Direction::Callees => Direction::Callers,
        };
        reset_to_root(&mut hierarchy.rows);
        hierarchy.selected = 0;
        self.expand_exact_row(0, cx);
        cx.notify();
    }

    pub(super) fn render_direction_switch(
        hierarchy: &HierarchyWindow,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !hierarchy.exact || hierarchy.kind != HierarchyKind::Calls {
            return None;
        }
        let option = |label: &'static str, on: bool| {
            div()
                .id(label)
                .px(px(6.))
                .rounded(px(4.))
                .when(on, |d| {
                    d.bg(theme::ui(cx, UiRole::Selected))
                        .text_color(theme::ui(cx, UiRole::Text))
                })
                .when(!on, |d| {
                    d.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        let current = this
                            .hierarchy_window
                            .as_ref()
                            .is_some_and(|h| h.direction == Direction::Callers);
                        if current != (label == "Callers") {
                            this.switch_call_direction(cx);
                        }
                    }),
                )
                .child(label)
        };
        let callers = hierarchy.direction == Direction::Callers;
        Some(
            div()
                .flex()
                .gap(px(4.))
                .text_size(px(11.))
                .child(option("Callers", callers))
                .child(option("Callees", !callers)),
        )
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::{Range, SymbolKind};

    use super::*;

    fn call(name: &str, uri: &str, line: u32) -> Item {
        let at = Position::new(line, 4);
        Item::Call(CallHierarchyItem {
            name: name.to_owned(),
            kind: SymbolKind::METHOD,
            tags: None,
            detail: Some("demo.Greeter".to_owned()),
            uri: uri.parse().unwrap(),
            range: Range::new(at, at),
            selection_range: Range::new(at, at),
            data: None,
        })
    }

    #[test]
    fn a_row_names_the_item_and_where_it_is() {
        let row = row(
            call(
                "greet",
                "jdt://contents/java.base/demo/Greeter.class?=demo/%3Cjava.lang%28Greeter.class",
                9,
            ),
            2,
            Expansion::Collapsed,
            &mut Texts::default(),
        );
        assert_eq!(row.label, "greet");
        assert_eq!(row.detail, "demo.Greeter · Greeter.class:10");
        assert_eq!((row.depth, row.location), (2, None));
    }

    #[test]
    fn items_are_the_same_by_place_not_by_detail() {
        let a = call("greet", "file:///a/G.java", 3);
        let b = call("greet", "file:///a/G.java", 3);
        let c = call("greet", "file:///a/G.java", 4);
        assert!(a.same(&b));
        assert!(!a.same(&c));
    }

    #[test]
    fn switching_direction_keeps_only_the_folded_root() {
        let mut texts = Texts::default();
        let mut rows = vec![
            row(
                call("greet", "file:///a/G.java", 3),
                0,
                Expansion::Expanded,
                &mut texts,
            ),
            row(
                call("main", "file:///a/M.java", 5),
                1,
                Expansion::Collapsed,
                &mut texts,
            ),
        ];
        reset_to_root(&mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].expansion, Expansion::Collapsed);
    }
}
