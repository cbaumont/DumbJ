use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dumbj_actions::Action;
use dumbj_settings::UiRole;
use dumbj_vcs::graph::{self, GraphRow};
use dumbj_vcs::{Change, Commit, LogOptions, Repository};
use gpui::{
    AppContext, Context, Div, IntoElement, MouseButton, ScrollStrategy, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use super::Workspace;
use super::diff::DiffSides;
use crate::file_tree::status_role;
use crate::theme;

const WINDOW_HEIGHT: f32 = 300.;
const ROW_HEIGHT: f32 = 22.;
const LANE_WIDTH: f32 = 12.;
const MAX_LANES: usize = 12;
const COMMIT_LIMIT: usize = 2000;
const SHORT_ID: usize = 10;
/// The id of git's empty tree, the base of a root commit's diff.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const LANE_ROLES: [UiRole; 5] = [
    UiRole::Info,
    UiRole::Success,
    UiRole::Warning,
    UiRole::Accent,
    UiRole::Error,
];

/// Parses the filter field: `branch:`, `author:`, `path:`, `since:`, `until:` and
/// `lines:FROM-TO` (one-based, inclusive) tokens; everything else is message text.
fn parse_filter(text: &str) -> LogOptions {
    let mut options = LogOptions {
        limit: COMMIT_LIMIT,
        ..LogOptions::default()
    };
    let mut words = Vec::new();
    for token in text.split_whitespace() {
        let field = token.split_once(':').filter(|(_, value)| !value.is_empty());
        match field {
            Some(("branch", value)) => options.revision = Some(value.to_owned()),
            Some(("author", value)) => options.author = Some(value.to_owned()),
            Some(("path", value)) => options.path = Some(PathBuf::from(value)),
            Some(("since", value)) => options.since = Some(value.to_owned()),
            Some(("until", value)) => options.until = Some(value.to_owned()),
            Some(("lines", value)) => {
                let range = value.split_once('-').and_then(|(from, to)| {
                    let (from, to) = (from.parse::<usize>().ok()?, to.parse::<usize>().ok()?);
                    (from >= 1 && to >= from).then(|| from - 1..to)
                });
                options.lines = range;
            }
            _ => words.push(token),
        }
    }
    if !words.is_empty() {
        options.text = Some(words.join(" "));
    }
    options
}

/// Whether the filtered log is a connected history that a graph can describe.
fn shows_graph(options: &LogOptions) -> bool {
    options.author.is_none()
        && options.text.is_none()
        && options.path.is_none()
        && options.since.is_none()
        && options.until.is_none()
}

/// `2026-11-16 07:10` for seconds since the Unix epoch, in UTC.
fn format_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

fn short(id: &str) -> String {
    id.chars().take(SHORT_ID).collect()
}

struct LoadedLog {
    commits: Vec<Commit>,
    graph: Vec<GraphRow>,
}

fn load_log(
    repository: &Repository,
    options: &LogOptions,
    cancelled: &AtomicBool,
) -> Result<Option<LoadedLog>, String> {
    let commits = repository
        .log(options, cancelled)
        .map_err(|error| error.to_string())?;
    Ok(commits.map(|commits| {
        let graph = if shows_graph(options) {
            graph::layout(&commits)
        } else {
            Vec::new()
        };
        LoadedLog { commits, graph }
    }))
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags, not a state machine"
)]
pub(super) struct GitWindow {
    options: LogOptions,
    commits: Vec<Commit>,
    graph: Vec<GraphRow>,
    selected: usize,
    scroll: UniformListScrollHandle,
    files: Vec<Change>,
    /// The commit `files` belongs to.
    files_of: Option<String>,
    file_selected: usize,
    files_scroll: UniformListScrollHandle,
    /// Whether the keys go to the file list, not the commit list.
    in_files: bool,
    loading: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    files_task: Option<Task<()>>,
    pub(super) visible: bool,
    pub(super) hidden_by_hide_all: bool,
}

impl Drop for GitWindow {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Workspace {
    fn git_window_shown(&self) -> bool {
        self.git_window.as_ref().is_some_and(|git| git.visible)
    }

    pub(super) fn git_filter_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.git_window_shown() && self.git_filter.read(cx).focus_handle().is_focused(window)
    }

    pub(super) fn git_window_focused(&self, window: &Window) -> bool {
        self.git_window_shown() && self.git_focus.is_focused(window)
    }

    /// Shows the Git window and focuses it, or hides it when it already has focus.
    pub(super) fn toggle_git_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        if self.git_window_focused(window) {
            if let Some(git) = self.git_window.as_mut() {
                git.visible = false;
            }
            self.focus_fallback(window, cx);
            cx.notify();
            return;
        }
        if self.git_window.is_none() {
            self.show_log("", window, cx);
            return;
        }
        if let Some(git) = self.git_window.as_mut() {
            git.visible = true;
        }
        let focus = self.git_focus.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Opens the Git window on the log that `filter` describes.
    fn show_log(&mut self, filter: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.git_filter
            .update(cx, |input, cx| input.set_text(filter, false, cx));
        self.git_window = Some(GitWindow {
            options: parse_filter(filter),
            commits: Vec::new(),
            graph: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            files: Vec::new(),
            files_of: None,
            file_selected: 0,
            files_scroll: UniformListScrollHandle::new(),
            in_files: false,
            loading: true,
            cancel: Arc::default(),
            task: None,
            files_task: None,
            visible: true,
            hidden_by_hide_all: false,
        });
        let focus = self.git_focus.clone();
        window.focus(&focus, cx);
        self.reload_log(cx);
    }

    fn reload_log(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(git)) = (self.repository.clone(), self.git_window.as_mut())
        else {
            return;
        };
        git.cancel.store(true, Ordering::Relaxed);
        git.cancel = Arc::default();
        git.loading = true;
        let (options, cancel) = (git.options.clone(), git.cancel.clone());
        git.task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load_log(&repository, &options, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| this.log_loaded(loaded, cx));
        }));
        cx.notify();
    }

    fn log_loaded(&mut self, loaded: Result<Option<LoadedLog>, String>, cx: &mut Context<Self>) {
        let Some(git) = self.git_window.as_mut() else {
            return;
        };
        match loaded {
            Ok(Some(loaded)) => {
                let selected = git
                    .commits
                    .get(git.selected)
                    .map(|commit| commit.id.clone());
                git.selected = selected
                    .and_then(|id| loaded.commits.iter().position(|commit| commit.id == id))
                    .unwrap_or(0);
                git.commits = loaded.commits;
                git.graph = loaded.graph;
                git.loading = false;
            }
            Ok(None) => return,
            Err(message) => {
                tracing::warn!(%message, "cannot read the log");
                git.commits.clear();
                git.graph.clear();
                git.loading = false;
                self.status_message = Some(message);
            }
        }
        self.load_commit_files(cx);
        cx.notify();
    }

    /// Lists the files the selected commit changed against its first parent.
    fn load_commit_files(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(git)) = (self.repository.clone(), self.git_window.as_mut())
        else {
            return;
        };
        let Some(commit) = git.commits.get(git.selected) else {
            git.files.clear();
            git.files_of = None;
            return;
        };
        if git.files_of.as_deref() == Some(commit.id.as_str()) {
            return;
        }
        let id = commit.id.clone();
        let base = commit
            .parents
            .first()
            .cloned()
            .unwrap_or_else(|| EMPTY_TREE.to_owned());
        let cancel = git.cancel.clone();
        git.files_task = Some(cx.spawn(async move |this, cx| {
            let head = id.clone();
            let files = cx
                .background_spawn(async move {
                    repository
                        .changed_files(&base, Some(&head), &cancel)
                        .inspect_err(|error| tracing::warn!(%error, "cannot list a commit's files"))
                        .ok()
                        .flatten()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let (Some(git), Some(files)) = (this.git_window.as_mut(), files) else {
                    return;
                };
                let wanted = git.options.path.clone();
                git.file_selected = wanted
                    .and_then(|path| files.iter().position(|change| change.path == path))
                    .unwrap_or(0);
                git.files = files;
                git.files_of = Some(id);
                cx.notify();
            });
        }));
    }

    fn apply_git_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.git_filter.read(cx).text().to_owned();
        if let Some(git) = self.git_window.as_mut() {
            git.options = parse_filter(&filter);
            git.in_files = false;
        }
        let focus = self.git_focus.clone();
        window.focus(&focus, cx);
        self.reload_log(cx);
    }

    pub(super) fn focus_git_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_filter
            .update(cx, crate::text_input::TextInput::select_all);
        let handle = self.git_filter.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(super) fn git_filter_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Enter => self.apply_git_filter(window, cx),
            Action::Escape => {
                let focus = self.git_focus.clone();
                window.focus(&focus, cx);
                cx.notify();
            }
            _ => {
                self.git_filter
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
            }
        }
    }

    /// The history of the active file, or of the lines its selection touches.
    pub(super) fn show_file_history(
        &mut self,
        selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(tab), Some(repository)) = (self.active_tab(), self.repository.as_ref()) else {
            return;
        };
        let Ok(relative) = tab.path.strip_prefix(repository.workdir()) else {
            return;
        };
        let mut filter = format!("path:{}", relative.display());
        if selection {
            let (first, last) = tab.editor.read(cx).selected_lines();
            filter = format!("lines:{first}-{last} {filter}");
        }
        self.show_log(&filter, window, cx);
    }

    fn open_commit_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repository), Some(git)) = (self.repository.as_ref(), self.git_window.as_mut())
        else {
            return;
        };
        let (Some(commit), Some(change)) = (git.commits.get(git.selected), git.files.get(index))
        else {
            return;
        };
        git.file_selected = index;
        let workdir = repository.workdir();
        let base = commit
            .parents
            .first()
            .map_or_else(|| EMPTY_TREE.to_owned(), |parent| short(parent));
        let sides = DiffSides {
            path: workdir.join(&change.path),
            old_path: change.old_path.as_ref().map(|path| workdir.join(path)),
            base,
            head: Some(short(&commit.id)),
        };
        self.show_diff(sides, window, cx);
    }

    pub(super) fn git_window_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(git) = self.git_window.as_mut() else {
            return;
        };
        let (count, current) = if git.in_files {
            (git.files.len(), git.file_selected)
        } else {
            (git.commits.len(), git.selected)
        };
        let last = count.saturating_sub(1);
        let target = match action {
            Action::MoveUp => Some(current.saturating_sub(1)),
            Action::MoveDown => Some(current + 1),
            Action::MovePageUp => Some(current.saturating_sub(10)),
            Action::MovePageDown => Some(current + 10),
            Action::MoveTextStart | Action::MoveLineStart => Some(0),
            Action::MoveTextEnd | Action::MoveLineEnd => Some(last),
            _ => None,
        };
        if let Some(target) = target {
            let target = target.min(last);
            if git.in_files {
                git.file_selected = target;
                git.files_scroll
                    .scroll_to_item(target, ScrollStrategy::Nearest);
            } else {
                git.selected = target;
                git.scroll.scroll_to_item(target, ScrollStrategy::Nearest);
                self.load_commit_files(cx);
            }
            cx.notify();
            return;
        }
        match action {
            Action::Tab | Action::MoveRight | Action::MoveLeft | Action::Unindent => {
                git.in_files = match action {
                    Action::MoveRight => true,
                    Action::MoveLeft => false,
                    _ => !git.in_files,
                };
            }
            Action::Enter if git.in_files => {
                let index = git.file_selected;
                self.open_commit_file(index, window, cx);
            }
            Action::Enter => git.in_files = true,
            Action::Escape if git.in_files => git.in_files = false,
            Action::Escape => self.focus_fallback(window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn render_graph_cell(row: &GraphRow, lanes: usize, cx: &Context<Self>) -> Div {
        #[expect(clippy::cast_precision_loss, reason = "a handful of lanes")]
        let x = |lane: usize| LANE_WIDTH * lane as f32 + LANE_WIDTH / 2.;
        let color = |lane: usize| {
            let role = LANE_ROLES
                .get(lane % LANE_ROLES.len())
                .copied()
                .unwrap_or(UiRole::Info);
            theme::ui(cx, role)
        };
        let half = ROW_HEIGHT / 2.;
        let vertical = |lane: usize, top: f32, height: f32| {
            div()
                .absolute()
                .left(px(x(lane) - 1.))
                .top(px(top))
                .w(px(2.))
                .h(px(height))
                .bg(color(lane))
        };
        let horizontal = |lane: usize| {
            let (from, to) = (x(lane.min(row.lane)), x(lane.max(row.lane)));
            div()
                .absolute()
                .left(px(from))
                .top(px(half - 1.))
                .w(px(to - from))
                .h(px(2.))
                .bg(color(lane))
        };
        let mut cell = div()
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .w(px(x(lanes.min(MAX_LANES))))
            .overflow_hidden();
        for (top, _) in &row.through {
            cell = cell.child(vertical(*top, 0., ROW_HEIGHT));
        }
        for lane in &row.into {
            cell = cell.child(vertical(*lane, 0., half));
            if *lane != row.lane {
                cell = cell.child(horizontal(*lane));
            }
        }
        for lane in &row.out {
            cell = cell.child(vertical(*lane, half, half));
            if *lane != row.lane {
                cell = cell.child(horizontal(*lane));
            }
        }
        cell.child(
            div()
                .absolute()
                .left(px(x(row.lane) - 4.))
                .top(px(half - 4.))
                .size(px(8.))
                .rounded_full()
                .bg(color(row.lane)),
        )
    }

    fn render_commit_row(
        git: &GitWindow,
        index: usize,
        commit: &Commit,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == git.selected;
        let background = match (selected, focused && !git.in_files) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        let graph = git
            .graph
            .get(index)
            .map(|row| Self::render_graph_cell(row, graph::width(&git.graph), cx));
        let refs = commit.refs.iter().map(|name| {
            div()
                .flex_none()
                .px(px(5.))
                .rounded(px(4.))
                .bg(theme::ui(cx, UiRole::InputBackground))
                .text_color(theme::ui(cx, UiRole::Warning))
                .text_size(px(11.))
                .child(name.clone())
        });
        div()
            .id(("git-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .when_some(background, |row, role| row.bg(theme::ui(cx, role)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    let focus = this.git_focus.clone();
                    window.focus(&focus, cx);
                    if let Some(git) = this.git_window.as_mut() {
                        git.selected = index;
                        git.in_files = false;
                    }
                    this.load_commit_files(cx);
                    cx.notify();
                }),
            )
            .children(graph)
            .children(refs)
            .child(div().flex_1().truncate().child(commit.subject.clone()))
            .child(
                div()
                    .w(px(140.))
                    .flex_none()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(commit.author.clone()),
            )
            .child(
                div()
                    .w(px(120.))
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(format_time(commit.time)),
            )
    }

    fn render_commit_file(
        git: &GitWindow,
        index: usize,
        change: &Change,
        focused: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = index == git.file_selected;
        let background = match (selected, focused && git.in_files) {
            (true, true) => Some(UiRole::Selected),
            (true, false) => Some(UiRole::SelectedInactive),
            (false, _) => None,
        };
        let name = |path: &Path| {
            path.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        };
        let folder = change
            .path
            .parent()
            .map_or_else(String::new, |parent| parent.display().to_string());
        div()
            .id(("git-file", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .when_some(background, |row, role| row.bg(theme::ui(cx, role)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let focus = this.git_focus.clone();
                    window.focus(&focus, cx);
                    if let Some(git) = this.git_window.as_mut() {
                        git.file_selected = index;
                        git.in_files = true;
                    }
                    if event.click_count >= 2 {
                        this.open_commit_file(index, window, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::ui(cx, status_role(change.status)))
                    .child(name(&change.path)),
            )
            .child(
                div()
                    .truncate()
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(folder),
            )
    }

    fn render_commit_details(
        git: &GitWindow,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let files = uniform_list(
            "git-files",
            git.files.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(git) = this.git_window.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, git.files.get(index)?)))
                    .map(|(index, change)| {
                        Self::render_commit_file(git, index, change, focused, cx).into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&git.files_scroll)
        .size_full();
        let message = git.commits.get(git.selected).map(|commit| {
            let body = (!commit.body.is_empty()).then(|| {
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(commit.body.clone())
            });
            div()
                .id("git-message")
                .flex_none()
                .max_h(px(120.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(12.))
                .py(px(6.))
                .border_b_1()
                .border_color(theme::ui(cx, UiRole::Border))
                .child(commit.subject.clone())
                .children(body)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::ui(cx, UiRole::TextFaint))
                        .child(format!(
                            "{} · {} <{}> · {} UTC",
                            short(&commit.id),
                            commit.author,
                            commit.email,
                            format_time(commit.time)
                        )),
                )
        });
        div()
            .w(px(460.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .children(message)
            .child(div().flex_1().min_h_0().child(files))
    }

    pub(super) fn render_git_window(
        &self,
        git: &GitWindow,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.git_focus.is_focused(window);
        let rows = uniform_list(
            "git-rows",
            git.commits.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let Some(git) = this.git_window.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| Some((index, git.commits.get(index)?)))
                    .map(|(index, commit)| {
                        Self::render_commit_row(git, index, commit, focused, cx).into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&git.scroll)
        .size_full();
        let summary = match (git.loading, git.commits.len()) {
            (true, _) => "Loading…".to_owned(),
            (false, 0) => "No commits".to_owned(),
            (false, COMMIT_LIMIT) => format!("First {COMMIT_LIMIT} commits"),
            (false, 1) => "1 commit".to_owned(),
            (false, count) => format!("{count} commits"),
        };
        let title = div()
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child("Git")
            .child(
                div()
                    .w(px(420.))
                    .flex()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .text_color(theme::ui(cx, UiRole::Text))
                    .child(self.git_filter.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(summary),
            );
        div()
            .track_focus(&self.git_focus)
            .h(px(WINDOW_HEIGHT))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_t_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().child(rows))
                    .child(Self::render_commit_details(git, focused, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_field_splits_into_log_options() {
        let options = parse_filter("fix crash author:ana branch:origin/main path:src/A.java");
        assert_eq!(options.text.as_deref(), Some("fix crash"));
        assert_eq!(options.author.as_deref(), Some("ana"));
        assert_eq!(options.revision.as_deref(), Some("origin/main"));
        assert_eq!(options.path, Some(PathBuf::from("src/A.java")));
        assert_eq!(options.limit, COMMIT_LIMIT);
        assert!(!shows_graph(&options));
    }

    #[test]
    fn dates_and_line_ranges_are_fields_too() {
        let options = parse_filter("since:2026-01-01 until:2026-02-01 lines:3-5");
        assert_eq!(options.since.as_deref(), Some("2026-01-01"));
        assert_eq!(options.until.as_deref(), Some("2026-02-01"));
        assert_eq!(options.lines, Some(2..5));
        assert_eq!(parse_filter("lines:5-3").lines, None);
        assert_eq!(parse_filter("lines:0-3").lines, None);
    }

    #[test]
    fn an_empty_or_branch_only_filter_keeps_the_graph() {
        assert!(shows_graph(&parse_filter("")));
        assert!(shows_graph(&parse_filter("branch:main")));
        assert_eq!(parse_filter("author:").text.as_deref(), Some("author:"));
    }

    #[test]
    fn times_are_formatted_as_utc_dates() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_time(1_794_813_054), "2026-11-16 07:10");
        assert_eq!(format_time(-1), "1969-12-31 23:59");
    }
}
