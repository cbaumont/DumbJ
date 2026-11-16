//! The menu bar in the header and the editor's context menu: every entry is an action.

use dumbj_actions::Action;
use dumbj_settings::UiRole;
use gpui::{
    AnyElement, Bounds, Context, IntoElement, MouseButton, Pixels, Point, Window, anchored, canvas,
    div, point, prelude::*, px,
};

use super::Workspace;
use crate::theme;

const ROW_HEIGHT: f32 = 24.;

const BAR_MENUS: usize = 6;

/// `None` is a separator.
type Entries = &'static [Option<Action>];

const FILE: Entries = &[
    Some(Action::OpenFolder),
    Some(Action::RecentFiles),
    None,
    Some(Action::CloseTab),
    None,
    Some(Action::ShowMainMenu),
    Some(Action::ShowContextMenu),
];

const EDIT: Entries = &[
    Some(Action::Undo),
    Some(Action::Redo),
    None,
    Some(Action::Cut),
    Some(Action::Copy),
    Some(Action::Paste),
    Some(Action::SelectAll),
    Some(Action::ExtendSelection),
    Some(Action::ShrinkSelection),
    None,
    Some(Action::Find),
    Some(Action::FindNext),
    Some(Action::FindPrevious),
    Some(Action::FindInPath),
    None,
    Some(Action::DuplicateLine),
    Some(Action::DeleteLine),
    Some(Action::MoveLineUp),
    Some(Action::MoveLineDown),
    Some(Action::ToggleLineComment),
];

const VIEW: Entries = &[
    Some(Action::ActivateProjectToolWindow),
    Some(Action::ActivateReviewToolWindow),
    Some(Action::PullRequests),
    Some(Action::ActivateFindToolWindow),
    Some(Action::ActivateHierarchyToolWindow),
    Some(Action::ActivateGitToolWindow),
    Some(Action::HideAllToolWindows),
    Some(Action::ToggleSmartMode),
    None,
    Some(Action::CollapseRegion),
    Some(Action::ExpandRegion),
    Some(Action::CollapseAllRegions),
    Some(Action::ExpandAllRegions),
    None,
    Some(Action::NextTab),
    Some(Action::PreviousTab),
];

const NAVIGATE: Entries = &[
    Some(Action::SearchEverywhere),
    Some(Action::GotoClass),
    Some(Action::GotoFile),
    Some(Action::GotoSymbol),
    Some(Action::GotoLine),
    Some(Action::FileStructure),
    None,
    Some(Action::GotoDeclaration),
    Some(Action::GotoImplementation),
    Some(Action::GotoSuperMethod),
    Some(Action::FindUsages),
    Some(Action::ShowUsages),
    Some(Action::TypeHierarchy),
    Some(Action::CallHierarchy),
    None,
    Some(Action::Back),
    Some(Action::Forward),
    Some(Action::RecentLocations),
    None,
    Some(Action::ToggleBookmark),
    Some(Action::ToggleBookmarkWithMnemonic),
    Some(Action::ShowBookmarks),
];

const GIT: Entries = &[
    Some(Action::GitBranches),
    Some(Action::ShowDiff),
    Some(Action::Annotate),
    Some(Action::ShowFileHistory),
    Some(Action::ShowHistoryForSelection),
    None,
    Some(Action::NextChange),
    Some(Action::PreviousChange),
    Some(Action::RollbackChangedLines),
    None,
    Some(Action::NextDiff),
    Some(Action::PreviousDiff),
    Some(Action::FocusOppositeDiffPane),
];

const REVIEW: Entries = &[
    Some(Action::StartReview),
    Some(Action::ToggleFileViewed),
    Some(Action::NextUnviewedFile),
    Some(Action::AddReviewNote),
    Some(Action::ExportReviewNotes),
    None,
    Some(Action::PullRequestOverview),
    Some(Action::OpenPullRequestInBrowser),
    None,
    Some(Action::AddReviewComment),
    Some(Action::ReplyToThread),
    Some(Action::ResolveThread),
    Some(Action::ToggleThread),
    Some(Action::NextThread),
    Some(Action::PreviousThread),
    None,
    Some(Action::SubmitReview),
    Some(Action::ApproveReview),
    Some(Action::RequestChanges),
    None,
    Some(Action::CopyPermalink),
    Some(Action::PruneReviewRefs),
];

const BAR: [(&str, Entries); BAR_MENUS] = [
    ("File", FILE),
    ("Edit", EDIT),
    ("View", VIEW),
    ("Navigate", NAVIGATE),
    ("Git", GIT),
    ("Review", REVIEW),
];

const CONTEXT: Entries = &[
    Some(Action::GotoDeclaration),
    Some(Action::GotoImplementation),
    Some(Action::GotoSuperMethod),
    Some(Action::FindUsages),
    Some(Action::TypeHierarchy),
    Some(Action::CallHierarchy),
    None,
    Some(Action::Cut),
    Some(Action::Copy),
    Some(Action::Paste),
    None,
    Some(Action::AddReviewComment),
    Some(Action::ReplyToThread),
    Some(Action::ResolveThread),
    Some(Action::ToggleThread),
    Some(Action::AddReviewNote),
    Some(Action::ToggleFileViewed),
    None,
    Some(Action::ShowDiff),
    Some(Action::Annotate),
    Some(Action::ShowFileHistory),
    Some(Action::ShowHistoryForSelection),
    Some(Action::RollbackChangedLines),
    Some(Action::CopyPermalink),
    None,
    Some(Action::ToggleBookmark),
];

const KEYS: [Action; 6] = [
    Action::MoveUp,
    Action::MoveDown,
    Action::MoveLeft,
    Action::MoveRight,
    Action::Enter,
    Action::Escape,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Row {
    /// `None` is a separator.
    action: Option<Action>,
    enabled: bool,
}

pub(super) struct Menu {
    origin: Point<Pixels>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// The menu of the bar this one hangs from.
    bar: Option<usize>,
}

/// The entries that can run, without the separators that would then stand alone.
fn only_enabled(rows: &[Row]) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for row in rows {
        let separator = row.action.is_none();
        let after_separator = out.last().is_none_or(|last| last.action.is_none());
        if (separator && !after_separator) || (!separator && row.enabled) {
            out.push(*row);
        }
    }
    if out.last().is_some_and(|last| last.action.is_none()) {
        out.pop();
    }
    out
}

fn step(rows: &[Row], from: Option<usize>, forward: bool) -> Option<usize> {
    let count = rows.len();
    if count == 0 {
        return None;
    }
    let start = from.unwrap_or(if forward { count - 1 } else { 0 });
    (1..=count)
        .map(|offset| {
            if forward {
                (start + offset) % count
            } else {
                (start + count - offset) % count
            }
        })
        .find(|at| rows[*at].action.is_some() && rows[*at].enabled)
}

impl Workspace {
    pub(super) fn menu_open(&self) -> bool {
        self.menus.open.is_some()
    }

    /// The actions that take keys while a menu is open.
    pub(super) fn menu_keys(action: Action) -> bool {
        KEYS.contains(&action)
    }

    fn menu_rows(&self, entries: Entries, window: &Window, cx: &gpui::App) -> Vec<Row> {
        entries
            .iter()
            .map(|action| Row {
                action: *action,
                enabled: action.is_some_and(|action| self.is_enabled(action, window, cx)),
            })
            .collect()
    }

    fn open_bar_menu(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) {
        let (Some((_, entries)), Some(bounds)) = (
            BAR.get(index),
            self.menus.bar_bounds.get().get(index).copied(),
        ) else {
            return;
        };
        let rows = self.menu_rows(entries, window, cx);
        let selected = self.menus.open.as_ref().and_then(|menu| menu.selected);
        self.menus.open = Some(Menu {
            origin: point(bounds.left(), bounds.bottom()),
            selected: selected.and_then(|_| step(&rows, None, true)),
            rows,
            bar: Some(index),
        });
        cx.notify();
    }

    pub(super) fn open_main_menu(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.open_bar_menu(0, window, cx);
        if let Some(menu) = self.menus.open.as_mut() {
            menu.selected = step(&menu.rows, None, true);
        }
    }

    pub(super) fn open_context_menu_at_caret(&mut self, window: &Window, cx: &mut Context<Self>) {
        let position = self
            .active_editor()
            .and_then(|editor| editor.read(cx).caret_popup_position());
        if let Some(position) = position {
            self.open_context_menu(position, window, cx);
            if let Some(menu) = self.menus.open.as_mut() {
                menu.selected = step(&menu.rows, None, true);
            }
        }
    }

    pub(super) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let rows = only_enabled(&self.menu_rows(CONTEXT, window, cx));
        if rows.is_empty() {
            return;
        }
        self.menus.open = Some(Menu {
            origin: position,
            rows,
            selected: None,
            bar: None,
        });
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menus.open = None;
        cx.notify();
    }

    fn run_menu_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .menus
            .open
            .as_ref()
            .and_then(|menu| menu.rows.get(index));
        let Some(Row {
            action: Some(action),
            enabled: true,
        }) = row.copied()
        else {
            return;
        };
        self.close_menu(cx);
        self.perform(action, window, cx);
    }

    pub(super) fn menu_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menus.open.as_mut() else {
            return;
        };
        match action {
            Action::MoveUp => menu.selected = step(&menu.rows, menu.selected, false),
            Action::MoveDown => menu.selected = step(&menu.rows, menu.selected, true),
            Action::MoveLeft | Action::MoveRight => {
                if let Some(bar) = menu.bar {
                    let by = if action == Action::MoveRight {
                        1
                    } else {
                        BAR_MENUS - 1
                    };
                    self.open_bar_menu((bar + by) % BAR_MENUS, window, cx);
                }
            }
            Action::Enter => {
                if let Some(selected) = menu.selected {
                    self.run_menu_row(selected, window, cx);
                }
            }
            _ => self.close_menu(cx),
        }
        cx.notify();
    }

    fn bar_menu_at(&self, position: Point<Pixels>) -> Option<usize> {
        self.menus
            .bar_bounds
            .get()
            .iter()
            .position(|bounds| bounds.contains(&position))
    }

    pub(super) fn render_menu_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.menus.open.as_ref().and_then(|menu| menu.bar);
        let labels = BAR.iter().enumerate().map(|(index, (title, _))| {
            let bounds = self.menus.bar_bounds.clone();
            div()
                .id(("menu-bar", index))
                .relative()
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .text_color(theme::ui(cx, UiRole::TextMuted))
                .when(open == Some(index), |label| {
                    label.bg(theme::ui(cx, UiRole::Selected))
                })
                .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_bar_menu(index, window, cx);
                }))
                .child(*title)
                .child(
                    canvas(
                        move |label, _, _| {
                            let mut all = bounds.get();
                            if let Some(slot) = all.get_mut(index) {
                                *slot = label;
                            }
                            bounds.set(all);
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
        });
        div().flex().items_center().children(labels)
    }

    fn render_menu_row(
        &self,
        index: usize,
        row: Row,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(action) = row.action else {
            return div()
                .h(px(1.))
                .my(px(4.))
                .bg(theme::ui(cx, UiRole::Border))
                .into_any_element();
        };
        let text = if row.enabled {
            UiRole::Text
        } else {
            UiRole::TextFaint
        };
        div()
            .id(("menu-row", index))
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(32.))
            .px(px(12.))
            .text_color(theme::ui(cx, text))
            .when(selected, |item| item.bg(theme::ui(cx, UiRole::Selected)))
            .when(row.enabled && !selected, |item| {
                item.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.run_menu_row(index, window, cx);
                }),
            )
            .child(div().flex_none().child(action.title()))
            .child(
                div()
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .children(self.shortcut(action)),
            )
            .into_any_element()
    }

    fn menu_layer_pressed(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.menus.open.as_ref().and_then(|menu| menu.bar);
        match self.bar_menu_at(event.position) {
            Some(index) if open != Some(index) => self.open_bar_menu(index, window, cx),
            _ => self.close_menu(cx),
        }
    }

    pub(super) fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = menu
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.render_menu_row(index, *row, menu.selected == Some(index), cx))
            .collect();
        let panel = div()
            .min_w(px(220.))
            .flex()
            .flex_col()
            .py(px(4.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(rows);
        div()
            .id("menu-layer")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::menu_layer_pressed))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::menu_layer_pressed))
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    let open = this.menus.open.as_ref().and_then(|menu| menu.bar);
                    if let (Some(open), Some(index)) = (open, this.bar_menu_at(event.position))
                        && open != index
                    {
                        this.open_bar_menu(index, window, cx);
                    }
                }),
            )
            .child(
                anchored()
                    .position(menu.origin)
                    .snap_to_window()
                    .child(panel),
            )
    }
}

#[derive(Default)]
pub(super) struct Menus {
    pub(super) open: Option<Menu>,
    /// Where the labels of the menu bar were painted.
    bar_bounds: std::rc::Rc<std::cell::Cell<[Bounds<Pixels>; BAR_MENUS]>>,
}

#[cfg(test)]
mod tests {
    use dumbj_actions::Scope;

    use super::*;

    fn row(action: Option<Action>, enabled: bool) -> Row {
        Row { action, enabled }
    }

    #[test]
    fn every_window_action_is_in_the_menu_bar() {
        let in_bar: Vec<Action> = BAR
            .iter()
            .flat_map(|(_, entries)| entries.iter().flatten().copied())
            .collect();
        let missing: Vec<Action> = Action::all()
            .filter(|action| action.scope() != Scope::Text)
            .filter(|action| {
                action.toggle_bookmark_digit().is_none() && action.goto_bookmark_digit().is_none()
            })
            .filter(|action| {
                !matches!(
                    action,
                    Action::InsertSuggestion
                        | Action::SendComment
                        | Action::SendCommentNow
                        | Action::Unindent
                )
            })
            .filter(|action| !in_bar.contains(action))
            .collect();
        assert_eq!(missing, []);
    }

    #[test]
    fn no_action_is_listed_twice_in_the_bar() {
        let mut in_bar: Vec<Action> = BAR
            .iter()
            .flat_map(|(_, entries)| entries.iter().flatten().copied())
            .collect();
        let count = in_bar.len();
        in_bar.sort();
        in_bar.dedup();
        assert_eq!(in_bar.len(), count);
    }

    #[test]
    fn a_context_menu_drops_what_cannot_run_and_stray_separators() {
        let rows = [
            row(Some(Action::GotoDeclaration), false),
            row(None, false),
            row(Some(Action::Copy), true),
            row(None, false),
            row(Some(Action::AddReviewComment), false),
            row(None, false),
            row(Some(Action::Annotate), true),
            row(None, false),
            row(Some(Action::ToggleBookmark), false),
        ];
        let kept: Vec<Option<Action>> = only_enabled(&rows).iter().map(|r| r.action).collect();
        assert_eq!(kept, [Some(Action::Copy), None, Some(Action::Annotate)]);
    }

    #[test]
    fn arrow_keys_skip_separators_and_disabled_rows_and_wrap() {
        let rows = [
            row(Some(Action::Cut), false),
            row(Some(Action::Copy), true),
            row(None, false),
            row(Some(Action::Paste), true),
        ];
        assert_eq!(step(&rows, None, true), Some(1));
        assert_eq!(step(&rows, Some(1), true), Some(3));
        assert_eq!(step(&rows, Some(3), true), Some(1));
        assert_eq!(step(&rows, None, false), Some(3));
        assert_eq!(step(&rows, Some(1), false), Some(3));
    }
}
