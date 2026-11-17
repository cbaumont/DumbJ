use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_actions::Action;
use dumbj_settings::{PullRequestRef, Review, UiRole};
use dumbj_vcs::{Change, FileStatus, Repository};
use gpui::{
    AppContext, Context, IntoElement, MouseButton, ScrollStrategy, Task, UniformListScrollHandle,
    Window, div, prelude::*, px, svg, uniform_list,
};

use super::diff::DiffSides;
use super::file_name;
use super::popup::{popup_box, popup_layer};
use super::review_tree::{self, Node, Row};
use super::{LeftTool, ToolWindowState, Workspace};
use crate::file_tree::status_role;
use crate::theme;

const ROW_HEIGHT: f32 = 22.;
const INDENT: f32 = 14.;
const PROMPT_ROW_HEIGHT: f32 = 26.;
const SHORT_ID: usize = 10;

/// What a review compares, as the user wrote it: `base` (against the working tree),
/// `base..head`, or `base...head` and `base...` (from the merge base, as in `git diff`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RangeSpec {
    base: String,
    head: Option<String>,
    merge_base: bool,
}

fn parse_spec(text: &str) -> Option<RangeSpec> {
    let text = text.trim();
    let (base, head, merge_base) = if let Some((base, head)) = text.split_once("...") {
        (base, head, true)
    } else if let Some((base, head)) = text.split_once("..") {
        (base, head, false)
    } else {
        (text, "", false)
    };
    let valid = |revision: &str| {
        !revision.is_empty()
            && !revision.starts_with('-')
            && !revision.contains(char::is_whitespace)
    };
    if !valid(base) || !(head.is_empty() || valid(head)) {
        return None;
    }
    Some(RangeSpec {
        base: base.to_owned(),
        head: (!head.is_empty()).then(|| head.to_owned()),
        merge_base,
    })
}

struct Loaded {
    base: String,
    changes: Vec<Change>,
    /// What each changed file holds on the head side, to notice when it changes again.
    fingerprints: Vec<String>,
    head_commit: Option<String>,
}

fn load(
    repository: &Repository,
    spec: &RangeSpec,
    cancelled: &AtomicBool,
) -> Result<Option<Loaded>, String> {
    let known = |revision: &str| match repository.resolve(revision) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("Unknown revision: {revision}")),
        Err(error) => Err(error.to_string()),
    };
    known(&spec.base)?;
    let head_commit = match &spec.head {
        Some(head) => Some(
            repository
                .resolve(head)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Unknown revision: {head}"))?,
        ),
        None => None,
    };
    let base = if spec.merge_base {
        let head = spec.head.as_deref().unwrap_or("HEAD");
        let id = repository
            .merge_base(&spec.base, head)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{} and {head} have no common ancestor", spec.base))?;
        id.chars().take(SHORT_ID).collect()
    } else {
        spec.base.clone()
    };
    let changes = repository
        .changed_files(&base, spec.head.as_deref(), cancelled)
        .map_err(|error| error.to_string())?;
    let Some(mut changes) = changes else {
        return Ok(None);
    };
    changes.sort_by(|a, b| review_tree::tree_order(&a.path, &b.path));
    let paths: Vec<PathBuf> = changes.iter().map(|change| change.path.clone()).collect();
    let ids = repository
        .content_ids(spec.head.as_deref(), &paths, cancelled)
        .map_err(|error| error.to_string())?;
    Ok(ids.map(|ids| Loaded {
        base,
        changes,
        head_commit,
        fingerprints: ids
            .into_iter()
            .map(|id| id.unwrap_or_else(|| "deleted".to_owned()))
            .collect(),
    }))
}

pub(super) struct ReviewWindow {
    title: String,
    spec: RangeSpec,
    /// The revision on the left of every diff: `spec.base`, or the merge base's id.
    base: String,
    changes: Vec<Change>,
    fingerprints: Vec<String>,
    rows: Vec<Row>,
    collapsed: HashSet<PathBuf>,
    /// A row of `rows`.
    selected: usize,
    scroll: UniformListScrollHandle,
    loading: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl ReviewWindow {
    fn rebuild_rows(&mut self) {
        let paths: Vec<&Path> = self.changes.iter().map(|c| c.path.as_path()).collect();
        self.rows = review_tree::rows(&paths, &self.collapsed);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn selected_file(&self) -> Option<usize> {
        match self.rows.get(self.selected)?.node {
            Node::File(index) => Some(index),
            Node::Folder { .. } => None,
        }
    }

    fn select_row(&mut self, row: usize) {
        self.selected = row.min(self.rows.len().saturating_sub(1));
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }

    /// Selects the row of a changed file, opening the folders above it.
    fn select_file(&mut self, index: usize) {
        let Some(change) = self.changes.get(index) else {
            return;
        };
        let folded = self.collapsed.len();
        self.collapsed
            .retain(|folder| !change.path.starts_with(folder));
        if self.collapsed.len() != folded {
            self.rebuild_rows();
        }
        let row = self.rows.iter().position(|r| r.node == Node::File(index));
        if let Some(row) = row {
            self.select_row(row);
        }
    }

    fn toggle_folder(&mut self, row: usize) {
        let Some(Node::Folder {
            path, collapsed, ..
        }) = self.rows.get(row).map(|r| &r.node)
        else {
            return;
        };
        if *collapsed {
            self.collapsed.remove(path);
        } else {
            self.collapsed.insert(path.clone());
        }
        self.rebuild_rows();
    }
}

impl Drop for ReviewWindow {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Suggestion {
    spec: String,
    detail: String,
}

pub(super) struct ReviewPrompt {
    suggestions: Vec<Suggestion>,
    selected: usize,
    scroll: UniformListScrollHandle,
}

fn suggestions(repository: &Repository) -> Vec<Suggestion> {
    let branches = repository
        .branches()
        .inspect_err(|error| tracing::warn!(%error, "cannot list the branches"))
        .unwrap_or_default();
    let mut out = vec![Suggestion {
        spec: "HEAD".to_owned(),
        detail: "Uncommitted changes".to_owned(),
    }];
    for branch in branches.iter().filter(|branch| !branch.current) {
        out.push(Suggestion {
            spec: format!("{}...", branch.name),
            detail: format!("Everything since branching off {}", branch.name),
        });
    }
    for branch in branches.iter().filter(|branch| !branch.current) {
        out.push(Suggestion {
            spec: branch.name.clone(),
            detail: format!("Working tree against {}", branch.name),
        });
    }
    out
}

/// The specs the prompt offers for `query`: matching suggestions, then the query itself
/// when it is a range no suggestion spells.
fn offered(suggestions: &[Suggestion], query: &str) -> Vec<(String, String)> {
    let query = query.trim();
    let needle = query.to_lowercase();
    let mut out: Vec<(String, String)> = suggestions
        .iter()
        .filter(|suggestion| suggestion.spec.to_lowercase().contains(&needle))
        .map(|suggestion| (suggestion.spec.clone(), suggestion.detail.clone()))
        .collect();
    if parse_spec(query).is_some() && out.iter().all(|(spec, _)| spec != query) {
        out.push((query.to_owned(), "Revision or range as typed".to_owned()));
    }
    out
}

/// What the left tool window shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LeftContent {
    Project,
    /// The changed files of the review that belongs to the chosen tool window.
    Files,
    NoReview,
    PullList,
}

impl Workspace {
    pub(super) fn left_content(&self) -> LeftContent {
        let pull = self.review_pull().is_some();
        match self.left_tool {
            LeftTool::Project => LeftContent::Project,
            LeftTool::Review if self.review.is_some() && !pull => LeftContent::Files,
            LeftTool::Review => LeftContent::NoReview,
            LeftTool::Pulls if pull && !self.pull_list.browsing => LeftContent::Files,
            LeftTool::Pulls => LeftContent::PullList,
        }
    }

    /// The left side shows something that takes the review window's focus.
    pub(super) fn review_shown(&self) -> bool {
        self.project_window == ToolWindowState::Visible
            && matches!(
                self.left_content(),
                LeftContent::Files | LeftContent::NoReview
            )
    }

    pub(super) fn open_review_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        self.review_prompt = Some(ReviewPrompt {
            suggestions: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
        });
        self.review_input
            .update(cx, |input, cx| input.set_text("", false, cx));
        let handle = self.review_input.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { suggestions(&repository) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(prompt) = this.review_prompt.as_mut() {
                    prompt.suggestions = found;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn review_query_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.review_prompt.as_mut() {
            prompt.selected = 0;
            cx.notify();
        }
    }

    fn close_review_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.review_prompt = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    fn accept_review_prompt(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.review_prompt.take() else {
            return;
        };
        let query = self.review_input.read(cx).text().to_owned();
        if let Some((spec, _)) = offered(&prompt.suggestions, &query).into_iter().nth(index) {
            self.start_review(&spec, window, cx);
        } else {
            self.status_message = Some("Not a revision or range".to_owned());
            self.focus_fallback(window, cx);
        }
        cx.notify();
    }

    pub(super) fn review_prompt_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.review_input.read(cx).text().to_owned();
        let Some(prompt) = self.review_prompt.as_mut() else {
            return;
        };
        let last = offered(&prompt.suggestions, &query).len().saturating_sub(1);
        match action {
            Action::MoveUp => prompt.selected = prompt.selected.saturating_sub(1),
            Action::MoveDown => prompt.selected = (prompt.selected + 1).min(last),
            Action::MovePageUp => prompt.selected = prompt.selected.saturating_sub(10),
            Action::MovePageDown => prompt.selected = (prompt.selected + 10).min(last),
            Action::Enter => {
                let selected = prompt.selected;
                self.accept_review_prompt(selected, window, cx);
                return;
            }
            Action::Escape => {
                self.close_review_prompt(window, cx);
                return;
            }
            _ => {
                self.review_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
                return;
            }
        }
        prompt
            .scroll
            .scroll_to_item(prompt.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn start_review(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_review(text, None, window, cx);
    }

    /// Starts the review of the range `text`; `pull` is the pull request it belongs to.
    pub(super) fn begin_review(
        &mut self,
        text: &str,
        pull: Option<PullRequestRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(spec) = parse_spec(text) else {
            self.status_message = Some("Not a revision or range".to_owned());
            self.focus_fallback(window, cx);
            return;
        };
        let title = pull.as_ref().map_or_else(
            || text.trim().to_owned(),
            |pull| format!("#{} {}", pull.number, pull.title),
        );
        self.left_tool = if pull.is_some() {
            LeftTool::Pulls
        } else {
            LeftTool::Review
        };
        self.pull_list.browsing = false;
        self.review = Some(ReviewWindow {
            title,
            base: spec.base.clone(),
            spec,
            changes: Vec::new(),
            fingerprints: Vec::new(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            loading: true,
            cancel: Arc::default(),
            task: None,
        });
        let saved = self
            .review
            .as_ref()
            .map(|review| &review.spec)
            .zip(self.state.review.as_ref());
        let same =
            saved.is_some_and(|(spec, saved)| saved.base == spec.base && saved.head == spec.head);
        if !same {
            let spec = self.review.as_ref().map(|review| &review.spec);
            self.state.review = spec.map(|spec| Review::new(&spec.base, spec.head.as_deref()));
        }
        if let Some(saved) = self.state.review.as_mut()
            && (!same || saved.pull != pull)
        {
            saved.pull = pull;
            self.save_project_state(cx);
        }
        self.session.pull_threads = None;
        self.load_pull_threads(cx);
        self.project_window = ToolWindowState::Visible;
        let focus = self.review_focus.clone();
        window.focus(&focus, cx);
        self.refresh_review(cx);
    }

    /// Makes the viewed marks of a pull request's files those GitHub has. Waits until both
    /// the file list and GitHub's answer are there.
    pub(super) fn take_viewed_from_github(&mut self, cx: &mut Context<Self>) {
        if self.review.as_ref().is_none_or(|review| review.loading) {
            return;
        }
        let Some(viewed) = self.github_viewed() else {
            return;
        };
        let (Some(review), Some(saved)) = (&self.review, &mut self.state.review) else {
            return;
        };
        for (change, fingerprint) in review.changes.iter().zip(&review.fingerprints) {
            let on_github = viewed.iter().any(|path| Path::new(path) == change.path);
            saved.set_viewed(&change.path, fingerprint, on_github);
        }
        self.save_project_state(cx);
        cx.notify();
    }

    fn review_file_viewed(&self, index: usize) -> bool {
        let (Some(review), Some(saved)) = (&self.review, &self.state.review) else {
            return false;
        };
        let file = review
            .changes
            .get(index)
            .zip(review.fingerprints.get(index));
        file.is_some_and(|(change, fingerprint)| saved.is_viewed(&change.path, fingerprint))
    }

    /// The review file that actions act on: the selected row while the Review window has
    /// focus, otherwise the file of the active diff.
    fn review_target(&self, window: &Window) -> Option<usize> {
        let review = self.review.as_ref()?;
        if self.review_shown() && self.review_focus.is_focused(window) {
            return review.selected_file();
        }
        self.active_review_file()
    }

    pub(super) fn can_mark_viewed(&self, window: &Window) -> bool {
        self.review_target(window).is_some()
    }

    pub(super) fn toggle_file_viewed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.review_target(window) else {
            return;
        };
        let viewed = !self.review_file_viewed(index);
        let (Some(review), Some(saved)) = (&self.review, &mut self.state.review) else {
            return;
        };
        if let Some((change, fingerprint)) = review
            .changes
            .get(index)
            .zip(review.fingerprints.get(index))
        {
            saved.set_viewed(&change.path, fingerprint, viewed);
            let path = change.path.clone();
            self.push_viewed(&path, viewed, cx);
        }
        self.save_project_state(cx);
        cx.notify();
    }

    pub(super) fn next_unviewed_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(count) = self.review.as_ref().map(|review| review.changes.len()) else {
            return;
        };
        let start = self.review_target(window).map_or(0, |index| index + 1);
        let next = (0..count)
            .map(|step| (start + step) % count.max(1))
            .find(|index| !self.review_file_viewed(*index));
        if let Some(index) = next {
            self.open_review_file(index, window, cx);
        } else {
            self.status_message = Some("Every file is viewed".to_owned());
            cx.notify();
        }
    }

    /// Lists the review's changed files again; the selection stays on its file.
    pub(super) fn refresh_review(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(review)) = (self.repository.clone(), self.review.as_mut())
        else {
            return;
        };
        review.cancel.store(true, Ordering::Relaxed);
        review.cancel = Arc::default();
        let (spec, cancel) = (review.spec.clone(), review.cancel.clone());
        review.task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load(&repository, &spec, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| this.review_loaded(loaded, cx));
        }));
    }

    fn review_loaded(&mut self, loaded: Result<Option<Loaded>, String>, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        match loaded {
            Ok(Some(loaded)) => {
                let selected = review.selected_file().and_then(|index| {
                    let path = &review.changes.get(index)?.path;
                    loaded.changes.iter().position(|c| c.path == *path)
                });
                review.base = loaded.base;
                review.changes = loaded.changes;
                review.fingerprints = loaded.fingerprints;
                review.rebuild_rows();
                review.select_file(selected.unwrap_or(0));
                review.loading = false;
                match (review.spec.head.clone(), loaded.head_commit) {
                    (Some(head), Some(commit)) => self.index_head(&head, &commit, cx),
                    _ => self.drop_head_index(),
                }
                self.take_viewed_from_github(cx);
            }
            Ok(None) => {}
            Err(message) => {
                tracing::warn!(%message, "cannot load the review");
                self.status_message = Some(message);
                if review.loading {
                    self.review = None;
                    self.head_index = None;
                    self.left_tool = LeftTool::Project;
                } else {
                    review.changes.clear();
                    review.rebuild_rows();
                }
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_review_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown =
            self.project_window == ToolWindowState::Visible && self.left_tool == LeftTool::Review;
        if shown && self.review_focus.is_focused(window) {
            self.project_window = ToolWindowState::Hidden;
            self.focus_fallback(window, cx);
        } else {
            self.left_tool = LeftTool::Review;
            self.project_window = ToolWindowState::Visible;
            let focus = self.review_focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn open_review_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repository), Some(review)) = (self.repository.as_ref(), self.review.as_mut())
        else {
            return;
        };
        review.select_file(index);
        let Some(change) = review.changes.get(index) else {
            return;
        };
        let workdir = repository.workdir();
        let sides = DiffSides {
            path: workdir.join(&change.path),
            old_path: change.old_path.as_ref().map(|path| workdir.join(path)),
            base: review.base.clone(),
            head: review.spec.head.clone(),
        };
        self.show_diff(sides, window, cx);
        self.offer_line_actions(cx);
        self.refresh_note_marks(cx);
        self.apply_thread_blocks(cx);
    }

    /// Lets the gutters of the active diff offer a `+` on the line under the pointer.
    fn offer_line_actions(&self, cx: &mut Context<Self>) {
        let Some((tab, diff)) = self
            .active_tab()
            .and_then(|tab| Some((tab, tab.diff.as_ref()?)))
        else {
            return;
        };
        for editor in [&tab.editor, diff.left()] {
            editor.update(cx, |editor, cx| editor.set_line_action(true, cx));
        }
    }

    /// A comment on a pull request's line, a note on a local review's.
    pub(super) fn line_action_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = [Action::AddReviewComment, Action::AddReviewNote]
            .into_iter()
            .find(|action| self.is_enabled(*action, window, cx));
        if let Some(action) = action {
            self.perform(action, window, cx);
        }
    }

    /// From a diff that belongs to the review, opens the diff of the next (or previous)
    /// changed file. Returns whether there was one.
    pub(super) fn step_review_file(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.neighbour_review_file(forward) else {
            return false;
        };
        self.open_review_file(target, window, cx);
        true
    }

    fn neighbour_review_file(&self, forward: bool) -> Option<usize> {
        let (review, current) = (self.review.as_ref()?, self.active_review_file()?);
        let target = if forward {
            current + 1
        } else {
            current.checked_sub(1)?
        };
        (target < review.changes.len()).then_some(target)
    }

    pub(super) fn diff_in_review(&self, diff: &super::diff::DiffTab) -> bool {
        self.review
            .as_ref()
            .is_some_and(|review| diff.compares(&review.base, review.spec.head.as_deref()))
    }

    pub(super) fn review_file_path(&self, index: usize) -> Option<PathBuf> {
        Some(self.review.as_ref()?.changes.get(index)?.path.clone())
    }

    /// The review file whose diff is the active tab.
    pub(super) fn active_review_file(&self) -> Option<usize> {
        let (review, tab) = (self.review.as_ref()?, self.active_tab()?);
        if !self.diff_in_review(tab.diff.as_ref()?) {
            return None;
        }
        let relative: PathBuf = tab
            .path
            .strip_prefix(self.repository.as_ref()?.workdir())
            .ok()?
            .to_path_buf();
        review.changes.iter().position(|c| c.path == relative)
    }

    pub(super) fn review_window_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.left_content() == LeftContent::NoReview {
            match action {
                Action::Enter => self.open_review_prompt(window, cx),
                Action::Escape => self.focus_fallback(window, cx),
                _ => {}
            }
            return;
        }
        if action == Action::Backspace && self.review_pull().is_some() {
            self.browse_pulls(true, window, cx);
            return;
        }
        let Some(review) = self.review.as_mut() else {
            return;
        };
        let selected = review.selected;
        let node = review.rows.get(selected).map(|row| row.node.clone());
        let folded = match &node {
            Some(Node::Folder { collapsed, .. }) => Some(*collapsed),
            _ => None,
        };
        match action {
            Action::MoveUp => review.select_row(selected.saturating_sub(1)),
            Action::MoveDown => review.select_row(selected + 1),
            Action::MovePageUp => review.select_row(selected.saturating_sub(20)),
            Action::MovePageDown => review.select_row(selected + 20),
            Action::MoveTextStart | Action::MoveLineStart => review.select_row(0),
            Action::MoveTextEnd | Action::MoveLineEnd => review.select_row(usize::MAX),
            Action::MoveRight if folded == Some(true) => review.toggle_folder(selected),
            Action::MoveRight if folded == Some(false) => review.select_row(selected + 1),
            Action::MoveLeft if folded == Some(false) => review.toggle_folder(selected),
            Action::MoveLeft => {
                if let Some(parent) = review_tree::parent_of(&review.rows, selected) {
                    review.select_row(parent);
                }
            }
            Action::Enter => match node {
                Some(Node::File(index)) => self.open_review_file(index, window, cx),
                Some(Node::Folder { .. }) => review.toggle_folder(selected),
                None => {}
            },
            Action::Escape => self.focus_fallback(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn render_review_node(
        review: &ReviewWindow,
        row: &Row,
        viewed: bool,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let icon = |path: &'static str, role: UiRole| {
            svg()
                .path(path)
                .size(px(14.))
                .flex_none()
                .text_color(theme::ui(cx, role))
        };
        match &row.node {
            Node::Folder {
                label, collapsed, ..
            } => {
                let (arrow, folder) = if *collapsed {
                    ("icons/chevron-right.svg", "icons/folder.svg")
                } else {
                    ("icons/chevron-down.svg", "icons/folder-open.svg")
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .min_w_0()
                    .child(icon(arrow, UiRole::TextFaint))
                    .child(icon(folder, UiRole::FolderIcon))
                    .child(div().truncate().child(label.clone()))
            }
            Node::File(file) => {
                let change = review.changes.get(*file);
                let status = change.map_or(FileStatus::Modified, |change| change.status);
                let name = change.map(|change| file_name(&change.path));
                let detail = change.and_then(|change| match (&change.old_path, change.status) {
                    (Some(old), _) => Some(format!("← {}", old.display())),
                    (None, FileStatus::Deleted) => Some("deleted".to_owned()),
                    (None, _) => None,
                });
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .min_w_0()
                    .when(viewed, |row| row.opacity(0.6))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .text_color(theme::ui(cx, UiRole::Success))
                            .child(if viewed { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme::ui(cx, status_role(status)))
                            .children(name),
                    )
                    .children(detail.map(|detail| {
                        div()
                            .truncate()
                            .text_color(theme::ui(cx, UiRole::TextFaint))
                            .child(detail)
                    }))
            }
        }
    }

    fn render_review_row(
        review: &ReviewWindow,
        index: usize,
        row: &Row,
        (focused, viewed): (bool, bool),
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == review.selected;
        let background = match (selected, focused) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        let file = match row.node {
            Node::File(file) => Some(file),
            Node::Folder { .. } => None,
        };
        let content = Self::render_review_node(review, row, viewed, cx);
        #[expect(clippy::cast_precision_loss, reason = "tree depth is small")]
        let indent = px(8. + INDENT * row.depth as f32);
        div()
            .id(("review-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .pl(indent)
            .pr(px(8.))
            .when_some(background, |row, role| row.bg(theme::ui(cx, role)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let focus = this.review_focus.clone();
                    window.focus(&focus, cx);
                    match file {
                        Some(file) if event.click_count >= 2 => {
                            this.open_review_file(file, window, cx);
                        }
                        Some(_) => {
                            if let Some(review) = this.review.as_mut() {
                                review.select_row(index);
                            }
                        }
                        None => {
                            if let Some(review) = this.review.as_mut() {
                                review.select_row(index);
                                review.toggle_folder(index);
                            }
                        }
                    }
                    cx.notify();
                }),
            )
            .child(content)
    }

    pub(super) fn render_review_window(
        &self,
        review: &ReviewWindow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.review_focus.is_focused(window);
        let rows = uniform_list(
            "review-rows",
            review.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(review) = this.review.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, review.rows.get(index)?)))
                    .map(|(index, row)| {
                        let viewed = match row.node {
                            Node::File(file) => this.review_file_viewed(file),
                            Node::Folder { .. } => false,
                        };
                        Self::render_review_row(review, index, row, (focused, viewed), cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&review.scroll)
        .size_full();
        let viewed = (0..review.changes.len())
            .filter(|index| self.review_file_viewed(*index))
            .count();
        let summary = match (review.loading, review.changes.len()) {
            (true, _) => "Loading…".to_owned(),
            (false, 0) => "No changes".to_owned(),
            (false, count) => format!("{viewed}/{count} viewed"),
        };
        let title = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(if self.review_pull().is_some() {
                div()
                    .id("review-back")
                    .flex_none()
                    .px(px(4.))
                    .rounded(px(4.))
                    .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.browse_pulls(true, window, cx)),
                    )
                    .child("‹")
                    .into_any_element()
            } else {
                "Review".into_any_element()
            })
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::Text))
                    .child(review.title.clone()),
            )
            .child(div().flex_1())
            .children(self.has_review_notes().then(|| {
                div()
                    .id("review-copy-notes")
                    .flex_none()
                    .px(px(6.))
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.export_review_notes(cx)))
                    .child("Copy notes")
            }))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(summary),
            );
        div()
            .track_focus(&self.review_focus)
            .w(super::TOOL_WINDOW_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .child(div().flex_1().min_h_0().child(rows))
    }

    pub(super) fn render_no_review(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = Action::StartReview.title();
        let hint = self.shortcut(Action::StartReview).map_or_else(
            || title.to_owned(),
            |shortcut| format!("{title} ({shortcut})"),
        );
        div()
            .track_focus(&self.review_focus)
            .w(super::TOOL_WINDOW_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(
                div()
                    .h(px(34.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child("Review"),
            )
            .child(
                div()
                    .px(px(12.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child("No local review in progress.")
                    .child(
                        div()
                            .id("review-start")
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(theme::ui(cx, UiRole::Selected))
                            .text_color(theme::ui(cx, UiRole::Text))
                            .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_review_prompt(window, cx);
                            }))
                            .child(hint),
                    ),
            )
    }

    fn render_review_prompt_row(
        index: usize,
        selected: bool,
        spec: &str,
        detail: &str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(("review-prompt-row", index))
            .w_full()
            .h(px(PROMPT_ROW_HEIGHT))
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
                    this.accept_review_prompt(index, window, cx);
                }),
            )
            .child(div().flex_none().child(spec.to_owned()))
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(detail.to_owned()),
            )
    }

    pub(super) fn render_review_prompt(
        &self,
        prompt: &ReviewPrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let offered = offered(&prompt.suggestions, self.review_input.read(cx).text());
        #[expect(
            clippy::cast_precision_loss,
            reason = "a popup shows at most a dozen rows"
        )]
        let list_height = px(PROMPT_ROW_HEIGHT * offered.len().min(12) as f32);
        let selected = prompt.selected;
        let rows = uniform_list(
            "review-prompt-rows",
            offered.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| Some((index, offered.get(index)?)))
                    .map(|(index, (spec, detail))| {
                        Self::render_review_prompt_row(index, index == selected, spec, detail, cx)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&prompt.scroll)
        .h(list_height);
        let popup = popup_box(cx)
            .w(px(640.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(Action::StartReview.title())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::ui(cx, UiRole::TextFaint))
                            .child("base · base..head · base...head"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.review_input.clone()),
            )
            .child(rows);
        popup_layer(
            "review-prompt-layer",
            140.,
            Workspace::close_review_prompt,
            popup,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(base: &str, head: Option<&str>, merge_base: bool) -> RangeSpec {
        RangeSpec {
            base: base.to_owned(),
            head: head.map(str::to_owned),
            merge_base,
        }
    }

    #[test]
    fn specs_follow_git_range_syntax() {
        assert_eq!(parse_spec(" main "), Some(spec("main", None, false)));
        assert_eq!(
            parse_spec("main..feature"),
            Some(spec("main", Some("feature"), false))
        );
        assert_eq!(
            parse_spec("main...feature"),
            Some(spec("main", Some("feature"), true))
        );
        assert_eq!(
            parse_spec("origin/main..."),
            Some(spec("origin/main", None, true))
        );
        assert_eq!(parse_spec("HEAD~2.."), Some(spec("HEAD~2", None, false)));
    }

    #[test]
    fn specs_that_git_would_misread_are_refused() {
        assert_eq!(parse_spec(""), None);
        assert_eq!(parse_spec("..main"), None);
        assert_eq!(parse_spec("--output=x"), None);
        assert_eq!(parse_spec("main..-x"), None);
        assert_eq!(parse_spec("a b"), None);
    }

    #[test]
    fn the_prompt_offers_matches_then_the_typed_range() {
        let suggestions = vec![
            Suggestion {
                spec: "HEAD".to_owned(),
                detail: String::new(),
            },
            Suggestion {
                spec: "main...".to_owned(),
                detail: String::new(),
            },
            Suggestion {
                spec: "main".to_owned(),
                detail: String::new(),
            },
        ];
        let specs = |query: &str| -> Vec<String> {
            offered(&suggestions, query)
                .into_iter()
                .map(|(spec, _)| spec)
                .collect()
        };
        assert_eq!(specs(""), ["HEAD", "main...", "main"]);
        assert_eq!(specs("MAI"), ["main...", "main", "MAI"]);
        assert_eq!(specs("main"), ["main...", "main"]);
        assert_eq!(specs("abc123..def"), ["abc123..def"]);
        assert_eq!(specs("not a range"), [] as [&str; 0]);
    }
}
