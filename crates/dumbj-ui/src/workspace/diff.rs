use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dumbj_settings::UiRole;
use dumbj_text::ByteOffset;
use dumbj_vcs::diff::{DiffOptions, Hunk, HunkKind, line_hunks_with, word_changes};
use gpui::{
    AppContext, Context, CursorStyle, Entity, EntityId, IntoElement, MouseButton, Subscription,
    Task, Window, div, prelude::*, px,
};

use super::{Tab, Workspace};
use crate::editor::{
    BlockClicked, ContextMenuRequested, DiffMarks, EditorView, LineActionClicked, MousePressed,
};
use crate::theme;

const HEADER_HEIGHT: gpui::Pixels = px(26.);
const CONTEXT_LINES: usize = 3;
const RETYPE_DELAY: Duration = Duration::from_millis(150);

pub(super) struct DiffTab {
    left: Entity<EditorView>,
    base: String,
    head: Option<String>,
    left_active: bool,
    options: DiffOptions,
    hunks: Arc<Vec<Hunk>>,
    right_marks: DiffMarks,
    computed: Option<(u64, u64)>,
    pending: Option<(u64, u64)>,
    tops: (f64, f64),
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DiffTab {
    pub(super) fn left(&self) -> &Entity<EditorView> {
        &self.left
    }

    /// Whether GitHub would take a comment on the lines `first..=last` of the active side.
    /// With whitespace ignored the hunks are not GitHub's, so nothing is ruled out.
    pub(super) fn takes_comment(&self, first: usize, last: usize) -> bool {
        self.options.ignore_whitespace
            || self.computed.is_none()
            || in_github_diff(&self.hunks, self.left_active, first, last)
    }

    pub(super) fn left_active(&self) -> bool {
        self.left_active
    }

    pub(super) fn right_marks(&self) -> DiffMarks {
        self.right_marks.clone()
    }

    pub(super) fn shows_working_file(&self) -> bool {
        self.head.is_none()
    }

    pub(super) fn compares(&self, base: &str, head: Option<&str>) -> bool {
        self.base == base && self.head.as_deref() == head
    }

    fn revision_editors(
        &self,
        right: &Entity<EditorView>,
    ) -> impl Iterator<Item = Entity<EditorView>> {
        let right = self.head.is_some().then(|| right.clone());
        std::iter::once(self.left.clone()).chain(right)
    }
}

pub(super) struct DiffSides {
    pub(super) path: PathBuf,
    pub(super) old_path: Option<PathBuf>,
    pub(super) base: String,
    pub(super) head: Option<String>,
}

impl DiffSides {
    pub(super) fn with_head_commit(path: PathBuf) -> Self {
        Self {
            path,
            old_path: None,
            base: "HEAD".to_owned(),
            head: None,
        }
    }
}

struct Computed {
    hunks: Vec<Hunk>,
    left: DiffMarks,
    right: DiffMarks,
}

fn compute(old: &str, new: &str, options: DiffOptions) -> Computed {
    let hunks = line_hunks_with(old, new, options);
    let (left, right) = marks(old, new, &hunks);
    Computed { hunks, left, right }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut at = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let start = at;
            at += line.len();
            start
        })
        .collect()
}

fn marks(old: &str, new: &str, hunks: &[Hunk]) -> (DiffMarks, DiffMarks) {
    let (old_starts, new_starts) = (line_starts(old), line_starts(new));
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let (mut left, mut right) = (DiffMarks::default(), DiffMarks::default());
    for hunk in hunks {
        match hunk.kind {
            HunkKind::Added => {
                left.gaps.push(hunk.old.start);
                right.lines.push((hunk.new.clone(), UiRole::Success));
            }
            HunkKind::Deleted => {
                left.lines.push((hunk.old.clone(), UiRole::TextMuted));
                right.gaps.push(hunk.new.start);
            }
            HunkKind::Modified => {
                left.lines.push((hunk.old.clone(), UiRole::Info));
                right.lines.push((hunk.new.clone(), UiRole::Info));
                for (old_line, new_line) in hunk.old.clone().zip(hunk.new.clone()) {
                    let (Some(old_text), Some(new_text)) =
                        (old_lines.get(old_line), new_lines.get(new_line))
                    else {
                        continue;
                    };
                    let (old_words, new_words) = word_changes(old_text, new_text);
                    let shifted = |starts: &[usize], line: usize, word: std::ops::Range<usize>| {
                        let base = starts.get(line).copied().unwrap_or(0);
                        ByteOffset(base + word.start)..ByteOffset(base + word.end)
                    };
                    left.words.extend(
                        old_words
                            .into_iter()
                            .map(|word| shifted(&old_starts, old_line, word)),
                    );
                    right.words.extend(
                        new_words
                            .into_iter()
                            .map(|word| shifted(&new_starts, new_line, word)),
                    );
                }
            }
        }
    }
    (left, right)
}

/// The line on the other side that sits level with `line` on this one: unchanged lines keep
/// their distance to the hunk before them, and a position inside a hunk maps to the same
/// fraction of the hunk's other half.
#[expect(
    clippy::cast_precision_loss,
    reason = "line numbers are far below 2^52"
)]
fn opposite_line(hunks: &[Hunk], line: f64, from_old: bool) -> f64 {
    let (mut from_at, mut to_at) = (0, 0);
    for hunk in hunks {
        let (from, to) = if from_old {
            (&hunk.old, &hunk.new)
        } else {
            (&hunk.new, &hunk.old)
        };
        if line < from.start as f64 {
            break;
        }
        if line < from.end as f64 {
            let fraction = (line - from.start as f64) / from.len() as f64;
            return to.start as f64 + fraction * to.len() as f64;
        }
        (from_at, to_at) = (from.end, to.end);
    }
    to_at as f64 + (line - from_at as f64)
}

const GITHUB_CONTEXT: usize = 3;

/// Whether GitHub's diff shows the one-based lines `first..=last` of a side in one piece,
/// which is what it needs to take a comment on them.
fn in_github_diff(hunks: &[Hunk], old_side: bool, first: usize, last: usize) -> bool {
    let (first, last) = (first.saturating_sub(1), last.saturating_sub(1));
    let mut shown: Option<Range<usize>> = None;
    for hunk in hunks {
        let lines = if old_side { &hunk.old } else { &hunk.new };
        let around = lines.start.saturating_sub(GITHUB_CONTEXT)..lines.end + GITHUB_CONTEXT;
        match &mut shown {
            Some(piece) if around.start <= piece.end => piece.end = around.end,
            Some(piece) if piece.start <= first && last < piece.end => return true,
            _ => shown = Some(around),
        }
    }
    shown.is_some_and(|piece| piece.start <= first && last < piece.end)
}

fn neighbour(hunks: &[Hunk], line: usize, old_side: bool, forward: bool) -> Option<usize> {
    let side = |hunk: &Hunk| {
        if old_side {
            hunk.old.clone()
        } else {
            hunk.new.clone()
        }
    };
    if forward {
        hunks
            .iter()
            .map(|hunk| side(hunk).start)
            .find(|start| *start > line)
    } else {
        hunks
            .iter()
            .rev()
            .map(side)
            .filter(|lines| !lines.contains(&line))
            .map(|lines| lines.start)
            .find(|start| *start < line)
    }
}

fn revision_label(revision: &str) -> String {
    let pull = revision
        .strip_prefix("refs/dumbj/pr/")
        .and_then(|rest| rest.split_once('/'));
    match pull {
        Some((number, "head")) => format!("#{number}"),
        Some((number, side)) => format!("#{number} {side}"),
        None => revision.to_owned(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl Workspace {
    fn diff_index(&self, left: EntityId) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            tab.diff
                .as_ref()
                .is_some_and(|diff| diff.left.entity_id() == left)
        })
    }

    pub(super) fn active_diff(&self) -> Option<&DiffTab> {
        self.active_tab()?.diff.as_ref()
    }

    pub(super) fn in_revision(&self) -> bool {
        self.active_diff()
            .is_some_and(|diff| diff.left_active || diff.head.is_some())
    }

    pub(super) fn diff_target(&self, cx: &gpui::App) -> Option<PathBuf> {
        self.repository.as_ref()?;
        let project = self.project.as_ref()?;
        project.tree.read(cx).selected_file().map(Path::to_path_buf)
    }

    pub(super) fn show_diff(
        &mut self,
        sides: DiffSides,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.path == sides.path
                && tab
                    .diff
                    .as_ref()
                    .is_some_and(|diff| diff.base == sides.base && diff.head == sides.head)
        }) {
            self.activate_tab(index, window, cx);
            return;
        }
        let DiffSides {
            path,
            old_path,
            base,
            head,
        } = sides;
        let right = match &head {
            Some(head) => {
                let right = if let Some(at_head) = self.head_path(head, &path) {
                    cx.new(|cx| EditorView::open(at_head, cx))
                } else {
                    let (path, repository, head) = (path.clone(), repository.clone(), head.clone());
                    cx.new(|cx| EditorView::open_revision(path, repository, head, cx))
                };
                self.subscribe_revision_editor(&right, window, cx);
                right
            }
            None => self.editor_for(&path, window, cx),
        };
        let left = cx.new(|cx| {
            let path = old_path.unwrap_or_else(|| path.clone());
            EditorView::open_revision(path, repository, base.clone(), cx)
        });
        self.subscribe_revision_editor(&left, window, cx);
        let id = left.entity_id();
        let (left_focus, right_focus) = (
            left.read(cx).focus_handle().clone(),
            right.read(cx).focus_handle().clone(),
        );
        let subscriptions = vec![
            cx.observe(&left, move |this, _, cx| this.diff_changed(id, cx)),
            cx.observe(&right, move |this, _, cx| this.diff_changed(id, cx)),
            cx.on_focus(&left_focus, window, move |this, _, cx| {
                this.set_diff_side(id, true, cx);
            }),
            cx.on_focus(&right_focus, window, move |this, _, cx| {
                this.set_diff_side(id, false, cx);
            }),
        ];
        let title = match &head {
            Some(head) => format!(
                "{} ({} → {})",
                file_name(&path),
                revision_label(&base),
                revision_label(head)
            ),
            None => format!("{} ({})", file_name(&path), revision_label(&base)),
        };
        self.tabs.push(Tab {
            title,
            path,
            editor: right,
            diff: Some(DiffTab {
                left,
                base,
                head,
                left_active: false,
                options: DiffOptions::default(),
                hunks: Arc::default(),
                right_marks: DiffMarks::default(),
                computed: None,
                pending: None,
                tops: (0.0, 0.0),
                task: None,
                _subscriptions: subscriptions,
            }),
        });
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    fn subscribe_revision_editor(
        &mut self,
        editor: &Entity<EditorView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.index_for(editor.read(cx).path()) {
            editor.update(cx, |editor, cx| editor.set_index(index, cx));
        }
        self.subscriptions.push(cx.subscribe_in(
            editor,
            window,
            |this, _, MousePressed(stroke), window, cx| {
                this.on_editor_mouse(*stroke, window, cx);
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            editor,
            window,
            |this, _, ContextMenuRequested(position), window, cx| {
                this.open_context_menu(*position, window, cx);
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            editor,
            window,
            |this, _, _: &LineActionClicked, window, cx| {
                this.line_action_clicked(window, cx);
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            editor,
            window,
            |this, _, BlockClicked(id), window, cx| {
                this.block_clicked(*id, window, cx);
            },
        ));
        self.subscriptions
            .push(cx.observe(editor, |_, _, cx| cx.notify()));
    }

    pub(super) fn index_revision_editors(&self, cx: &mut Context<Self>) {
        let in_diffs = self
            .tabs
            .iter()
            .filter_map(|tab| Some(tab.diff.as_ref()?.revision_editors(&tab.editor)))
            .flatten();
        let at_revisions = self
            .tabs
            .iter()
            .filter(|tab| dumbj_vcs::split_revision_path(&tab.path).is_some())
            .map(|tab| tab.editor.clone());
        let editors: Vec<_> = in_diffs.chain(at_revisions).collect();
        for editor in editors {
            if let Some(index) = self.index_for(editor.read(cx).path()) {
                editor.update(cx, |editor, cx| editor.set_index(index, cx));
            }
        }
    }

    fn set_diff_side(&mut self, left: EntityId, left_active: bool, cx: &mut Context<Self>) {
        let diff = self
            .diff_index(left)
            .and_then(|index| self.tabs.get_mut(index))
            .and_then(|tab| tab.diff.as_mut());
        if let Some(diff) = diff.filter(|diff| diff.left_active != left_active) {
            diff.left_active = left_active;
            cx.notify();
        }
    }

    pub(super) fn diff_changed(&mut self, left: EntityId, cx: &mut Context<Self>) {
        let Some(index) = self.diff_index(left) else {
            return;
        };
        let active = self.active == Some(index);
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let right = tab.editor.clone();
        let Some(diff) = tab.diff.as_mut() else {
            return;
        };
        let snapshots = (
            diff.left.read(cx).diff_snapshot(),
            right.read(cx).diff_snapshot(),
        );
        let (Some(old), Some(new)) = snapshots else {
            return;
        };
        let versions = (old.version(), new.version());
        if diff.computed != Some(versions) && diff.pending != Some(versions) {
            let delay = if diff.computed.is_some() {
                RETYPE_DELAY
            } else {
                Duration::ZERO
            };
            let options = diff.options;
            diff.pending = Some(versions);
            diff.task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let computed = cx
                    .background_spawn(async move {
                        compute(&old.to_string(), &new.to_string(), options)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.diff_computed(left, versions, computed, cx);
                });
            }));
        }
        if !active || diff.computed.is_none() {
            return;
        }
        let tops = (diff.left.read(cx).top_line(), right.read(cx).top_line());
        let moved = |a: f64, b: f64| (a - b).abs() > f64::EPSILON;
        if moved(tops.0, diff.tops.0) {
            let target = opposite_line(&diff.hunks, tops.0, true);
            right.update(cx, |editor, cx| editor.set_top_line(target, cx));
        } else if moved(tops.1, diff.tops.1) {
            let target = opposite_line(&diff.hunks, tops.1, false);
            diff.left
                .update(cx, |editor, cx| editor.set_top_line(target, cx));
        } else {
            return;
        }
        diff.tops = (diff.left.read(cx).top_line(), right.read(cx).top_line());
    }

    fn diff_computed(
        &mut self,
        left: EntityId,
        versions: (u64, u64),
        computed: Computed,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.diff_index(left) else {
            return;
        };
        let active = self.active == Some(index);
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let right = tab.editor.clone();
        let Some(diff) = tab.diff.as_mut() else {
            return;
        };
        if diff.pending != Some(versions) {
            return;
        }
        let first = diff.computed.is_none();
        diff.pending = None;
        diff.computed = Some(versions);
        diff.hunks = Arc::new(computed.hunks);
        diff.right_marks = computed.right;
        diff.left
            .update(cx, |editor, cx| editor.set_diff_marks(computed.left, cx));
        if active {
            let marks = diff.right_marks.clone();
            right.update(cx, |editor, cx| editor.set_diff_marks(marks, cx));
        }
        if let Some(hunk) = diff.hunks.first().filter(|_| first && active) {
            #[expect(clippy::cast_precision_loss, reason = "a line number")]
            let top = hunk.new.start.saturating_sub(CONTEXT_LINES) as f64;
            right.update(cx, |editor, cx| editor.set_top_line(top, cx));
            let opposite = opposite_line(&diff.hunks, top, false);
            diff.left
                .update(cx, |editor, cx| editor.set_top_line(opposite, cx));
            diff.tops = (diff.left.read(cx).top_line(), right.read(cx).top_line());
        }
        cx.notify();
    }

    pub(super) fn go_to_difference(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(diff), Some(editor)) = (self.active_diff(), self.active_editor()) else {
            return;
        };
        let line = editor.read(cx).caret_line_column().0.saturating_sub(1);
        if let Some(target) = neighbour(&diff.hunks, line, diff.left_active, forward) {
            editor.update(cx, |editor, cx| editor.go_to(target + 1, None, cx));
        } else if !self.step_review_file(forward, window, cx) {
            let edge = if forward { "last" } else { "first" };
            self.status_message = Some(format!("This is the {edge} difference"));
            cx.notify();
        }
    }

    pub(super) fn focus_opposite_diff_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let Some(diff) = &tab.diff else { return };
        let target = if diff.left_active {
            &tab.editor
        } else {
            &diff.left
        };
        let handle = target.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
    }

    fn toggle_ignore_whitespace(&mut self, left: EntityId, cx: &mut Context<Self>) {
        let diff = self
            .diff_index(left)
            .and_then(|index| self.tabs.get_mut(index))
            .and_then(|tab| tab.diff.as_mut());
        if let Some(diff) = diff {
            diff.options.ignore_whitespace = !diff.options.ignore_whitespace;
            diff.pending = None;
            diff.computed = Some((u64::MAX, u64::MAX));
            self.diff_changed(left, cx);
        }
    }

    pub(super) fn render_diff(
        tab: &Tab,
        diff: &DiffTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = diff.left.entity_id();
        let summary = match (diff.computed, diff.hunks.len()) {
            (None, _) => "Comparing…".to_owned(),
            (Some(_), 0) => "No differences".to_owned(),
            (Some(_), 1) => "1 difference".to_owned(),
            (Some(_), count) => format!("{count} differences"),
        };
        let ignoring = diff.options.ignore_whitespace;
        let toggle = div()
            .id("diff-ignore-whitespace")
            .px(px(8.))
            .rounded(px(4.))
            .cursor(CursorStyle::PointingHand)
            .when(ignoring, |toggle| {
                toggle
                    .bg(theme::ui(cx, UiRole::Selected))
                    .text_color(theme::ui(cx, UiRole::Text))
            })
            .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_ignore_whitespace(id, cx)))
            .child("Ignore whitespace");
        let header = |label: String, active: bool| {
            div()
                .h(HEADER_HEIGHT)
                .flex_none()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .bg(theme::ui(cx, UiRole::PanelBackground))
                .border_b_1()
                .border_color(theme::ui(cx, UiRole::Border))
                .text_size(px(12.))
                .text_color(theme::ui(
                    cx,
                    if active {
                        UiRole::Text
                    } else {
                        UiRole::TextMuted
                    },
                ))
                .child(label)
        };
        let pane = || div().flex_1().min_w_0().flex().flex_col();
        let left = pane()
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(header(revision_label(&diff.base), diff.left_active))
            .child(div().flex_1().min_h_0().child(diff.left.clone()));
        let right = pane()
            .child(
                header(
                    diff.head
                        .as_deref()
                        .map_or_else(|| "Working tree".to_owned(), revision_label),
                    !diff.left_active,
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(theme::ui(cx, UiRole::TextMuted))
                        .child(summary),
                )
                .child(toggle),
            )
            .child(div().flex_1().min_h_0().child(tab.editor.clone()));
        div().size_full().flex().child(left).child(right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(kind: HunkKind, old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> Hunk {
        Hunk { kind, old, new }
    }

    #[test]
    fn github_takes_comments_on_changes_and_three_lines_around() {
        let hunk = |old: Range<usize>, new: Range<usize>| Hunk {
            kind: HunkKind::Modified,
            old,
            new,
        };
        let hunks = [
            hunk(10..12, 10..11),
            hunk(16..16, 15..17),
            hunk(40..41, 41..42),
        ];
        assert!(in_github_diff(&hunks, false, 11, 11));
        assert!(in_github_diff(&hunks, false, 8, 8), "context above");
        assert!(!in_github_diff(&hunks, false, 7, 7));
        assert!(
            in_github_diff(&hunks, false, 9, 19),
            "two hunks whose context joins"
        );
        assert!(!in_github_diff(&hunks, false, 20, 39));
        assert!(
            !in_github_diff(&hunks, false, 19, 41),
            "two separate pieces"
        );
        assert!(in_github_diff(&hunks, true, 41, 41));
        assert!(
            in_github_diff(&hunks, true, 14, 16),
            "around a pure addition"
        );
        assert!(!in_github_diff(&[], false, 1, 1));
    }

    fn hunks() -> Vec<Hunk> {
        vec![
            hunk(HunkKind::Added, 2..2, 2..5),
            hunk(HunkKind::Modified, 6..8, 9..13),
            hunk(HunkKind::Deleted, 10..14, 15..15),
        ]
    }

    #[test]
    fn pull_request_refs_read_as_numbers() {
        assert_eq!(revision_label("refs/dumbj/pr/12/head"), "#12");
        assert_eq!(revision_label("refs/dumbj/pr/12/base"), "#12 base");
        assert_eq!(revision_label("main"), "main");
        assert_eq!(revision_label("refs/dumbj/pr/"), "refs/dumbj/pr/");
    }

    #[test]
    fn unchanged_lines_keep_their_offset_from_the_hunk_before() {
        let hunks = hunks();
        assert!((opposite_line(&hunks, 1.5, true) - 1.5).abs() < 1e-9);
        assert!((opposite_line(&hunks, 3.0, true) - 6.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 6.0, false) - 3.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 9.0, true) - 14.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 20.0, true) - 21.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 21.0, false) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_position_inside_a_hunk_maps_proportionally() {
        let hunks = hunks();
        assert!((opposite_line(&hunks, 7.0, true) - 11.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 10.0, false) - 6.5).abs() < 1e-9);
        assert!((opposite_line(&hunks, 3.5, false) - 2.0).abs() < 1e-9);
        assert!((opposite_line(&hunks, 12.0, true) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn next_and_previous_difference_on_each_side() {
        let hunks = hunks();
        assert_eq!(neighbour(&hunks, 0, false, true), Some(2));
        assert_eq!(neighbour(&hunks, 2, false, true), Some(9));
        assert_eq!(neighbour(&hunks, 15, false, true), None);
        assert_eq!(neighbour(&hunks, 11, false, false), Some(2));
        assert_eq!(neighbour(&hunks, 0, true, true), Some(2));
        assert_eq!(neighbour(&hunks, 7, true, true), Some(10));
        assert_eq!(neighbour(&hunks, 12, true, false), Some(6));
        assert_eq!(neighbour(&hunks, 2, true, false), None);
    }

    #[test]
    fn marks_colour_lines_and_changed_words() {
        let old = "a\nlet x = 1;\nb\ngone\nz\n";
        let new = "a\nlet x = 2;\nb\nz\nnew\n";
        let computed = compute(old, new, DiffOptions::default());
        assert_eq!(
            computed.left.lines,
            [(1..2, UiRole::Info), (3..4, UiRole::TextMuted)]
        );
        assert_eq!(
            computed.right.lines,
            [(1..2, UiRole::Info), (4..5, UiRole::Success)]
        );
        assert_eq!(computed.left.words, [ByteOffset(10)..ByteOffset(11)]);
        assert_eq!(computed.right.words, [ByteOffset(10)..ByteOffset(11)]);
        assert_eq!(computed.right.gaps, [3]);
        assert_eq!(computed.left.gaps, [5]);
    }
}
