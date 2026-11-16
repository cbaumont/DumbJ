//! The pull request list and opening a pull request as a review.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use dumbj_actions::Action;
use dumbj_review::{Client, PrFilter, PullRequestSummary, RepoId, ReviewDecision};
use dumbj_settings::{PullRequestRef, UiRole};
use dumbj_vcs::{Repository, pull_base_ref, pull_head_ref};
use gpui::{
    AppContext, Context, IntoElement, MouseButton, ScrollStrategy, Task, UniformListScrollHandle,
    Window, div, prelude::*, px, uniform_list,
};

use super::review::LeftContent;
use super::{LeftTool, ToolWindowState, Workspace};
use crate::theme;

const ROW_HEIGHT: f32 = 44.;
const DEBOUNCE: Duration = Duration::from_millis(300);
const FILTERS: [PrFilter; 4] = [
    PrFilter::ReviewRequested,
    PrFilter::Assigned,
    PrFilter::Authored,
    PrFilter::All,
];

/// The GitHub repository behind one of the project's remotes, and a signed-in client.
#[derive(Debug)]
pub(super) struct GitHub {
    remote: String,
    repo: RepoId,
    pub(super) client: Client,
}

pub(super) fn connect(repository: &Repository) -> Result<GitHub, String> {
    let mut remotes = repository.remotes().map_err(|error| error.to_string())?;
    remotes.sort_by_key(|remote| remote.name != "origin");
    let (remote, repo) = remotes
        .into_iter()
        .find_map(|remote| Some((remote.name, RepoId::from_remote_url(&remote.url)?)))
        .ok_or_else(|| "No remote of this repository is on a GitHub host".to_owned())?;
    let token = dumbj_review::token(&repo.host).map_err(|error| error.to_string())?;
    let client = Client::new(&repo.host, token);
    Ok(GitHub {
        remote,
        repo,
        client,
    })
}

/// What is known about GitHub and the pull request under review.
#[derive(Default)]
pub(super) struct Session {
    pub(super) github: Option<Arc<GitHub>>,
    pub(super) pull_threads: Option<super::threads::PullThreads>,
    /// Draft comments are on their way to GitHub.
    pub(super) sending: bool,
    /// The width the threads of each open diff side were last wrapped at.
    pub(super) wrapped_at: Vec<usize>,
}

impl GitHub {
    pub(super) fn repo(&self) -> &RepoId {
        &self.repo
    }
}

fn filter_label(filter: PrFilter) -> &'static str {
    match filter {
        PrFilter::ReviewRequested => "Review requested",
        PrFilter::Assigned => "Assigned",
        PrFilter::Authored => "Mine",
        PrFilter::All => "All",
    }
}

fn next_filter(filter: PrFilter, forward: bool) -> PrFilter {
    let at = FILTERS.iter().position(|f| *f == filter).unwrap_or(0);
    let step = if forward { 1 } else { FILTERS.len() - 1 };
    FILTERS[(at + step) % FILTERS.len()]
}

fn detail(pull: &PullRequestSummary) -> String {
    let decision = match pull.review_decision {
        Some(ReviewDecision::Approved) => " · approved",
        Some(ReviewDecision::ChangesRequested) => " · changes requested",
        _ => "",
    };
    let draft = if pull.is_draft { " · draft" } else { "" };
    let day: String = pull.updated_at.chars().take(10).collect();
    format!(
        "{} · {} → {} · {day}{draft}{decision}",
        pull.author, pull.head_ref, pull.base_ref
    )
}

enum Listing {
    Loading,
    Loaded,
    Failed(String),
}

pub(super) struct PullList {
    filter: PrFilter,
    pulls: Vec<PullRequestSummary>,
    listing: Listing,
    selected: usize,
    scroll: UniformListScrollHandle,
    task: Option<Task<()>>,
    asked: bool,
    /// The list is shown although a pull request is under review.
    pub(super) browsing: bool,
}

impl PullList {
    pub(super) fn new() -> Self {
        Self {
            filter: PrFilter::ReviewRequested,
            pulls: Vec::new(),
            listing: Listing::Loading,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            task: None,
            asked: false,
            browsing: false,
        }
    }
}

impl Workspace {
    fn pull_window_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.review_focus.is_focused(window)
            || self.pull_input.read(cx).focus_handle().is_focused(window)
    }

    fn toggle_pull_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown =
            self.project_window == ToolWindowState::Visible && self.left_tool == LeftTool::Pulls;
        if shown && self.pull_window_focused(window, cx) {
            self.project_window = ToolWindowState::Hidden;
            self.focus_fallback(window, cx);
        } else {
            self.left_tool = LeftTool::Pulls;
            self.project_window = ToolWindowState::Visible;
            self.focus_pull_window(window, cx);
        }
        cx.notify();
    }

    fn focus_pull_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.left_content() == LeftContent::PullList {
            let handle = self.pull_input.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
            if !self.pull_list.asked {
                self.load_pulls(false, cx);
            }
        } else {
            let focus = self.review_focus.clone();
            window.focus(&focus, cx);
        }
    }

    /// Shows the list in place of the files of the pull request under review, or back.
    pub(super) fn browse_pulls(
        &mut self,
        browsing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pull_list.browsing = browsing;
        self.focus_pull_window(window, cx);
        cx.notify();
    }

    /// Asks GitHub for the list again, after a pause when the user is typing.
    pub(super) fn load_pulls(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let text = self.pull_input.read(cx).text().to_owned();
        let Some(repository) = self.repository.clone() else {
            return;
        };
        let list = &mut self.pull_list;
        let (filter, known) = (list.filter, self.session.github.clone());
        list.listing = Listing::Loading;
        list.asked = true;
        list.task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(DEBOUNCE).await;
            }
            let listed = cx
                .background_spawn(async move {
                    let github = match known {
                        Some(github) => github,
                        None => Arc::new(connect(&repository)?),
                    };
                    let pulls = github
                        .client
                        .pull_requests(&github.repo, filter, &text)
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>((github, pulls))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let list = &mut this.pull_list;
                match listed {
                    Ok((github, pulls)) => {
                        this.session.github = Some(github);
                        list.pulls = pulls;
                        list.listing = Listing::Loaded;
                    }
                    Err(message) => {
                        list.pulls.clear();
                        list.listing = Listing::Failed(message);
                    }
                }
                list.selected = 0;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn pull_list_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = &mut self.pull_list;
        let end = list.pulls.len().saturating_sub(1);
        match action {
            Action::MoveUp => list.selected = list.selected.saturating_sub(1),
            Action::MoveDown => list.selected = (list.selected + 1).min(end),
            Action::MovePageUp => list.selected = list.selected.saturating_sub(10),
            Action::MovePageDown => list.selected = (list.selected + 10).min(end),
            Action::Tab | Action::Unindent => {
                list.filter = next_filter(list.filter, action == Action::Tab);
                self.load_pulls(false, cx);
                return;
            }
            Action::Enter => {
                let selected = list.selected;
                self.accept_pull(selected, window, cx);
                return;
            }
            Action::Escape => {
                if self.review_pull().is_some() {
                    self.browse_pulls(false, window, cx);
                } else {
                    self.focus_fallback(window, cx);
                }
                return;
            }
            _ => {
                self.pull_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
                return;
            }
        }
        list.scroll
            .scroll_to_item(list.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn accept_pull(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pull) = self.pull_list.pulls.get(index).cloned() {
            self.pull_list.selected = index;
            self.open_pull(pull, window, cx);
        }
        cx.notify();
    }

    /// Fetches the pull request into `refs/dumbj/pr/` and starts its review.
    fn open_pull(&mut self, pull: PullRequestSummary, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repository), Some(github)) =
            (self.repository.clone(), self.session.github.clone())
        else {
            return;
        };
        self.status_message = Some(format!("Fetching #{}…", pull.number));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (remote, number, base) =
                (github.remote.clone(), pull.number, pull.base_ref.clone());
            let fetched = cx
                .background_spawn(async move {
                    repository.fetch_pull(&remote, number, &base, &AtomicBool::new(false))
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match fetched {
                    Ok(_) => {
                        this.status_message = None;
                        let range =
                            format!("{}...{}", pull_base_ref(number), pull_head_ref(number));
                        let pull = PullRequestRef {
                            host: github.repo.host.clone(),
                            owner: github.repo.owner.clone(),
                            name: github.repo.name.clone(),
                            number,
                            title: pull.title,
                            url: pull.url,
                        };
                        this.begin_review(&range, Some(pull), window, cx);
                    }
                    Err(error) => {
                        this.status_message = Some(format!("Cannot fetch #{number}: {error}"));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
        cx.notify();
    }

    /// The pull request under review, if the review is of one.
    pub(super) fn review_pull(&self) -> Option<&PullRequestRef> {
        self.review.as_ref()?;
        self.state.review.as_ref()?.pull.as_ref()
    }

    pub(super) fn pull_action_enabled(&self, action: Action, popup_open: bool) -> bool {
        match action {
            Action::SendComment | Action::SendCommentNow | Action::InsertSuggestion => {
                self.compose_open()
            }
            _ if popup_open => false,
            Action::PullRequests | Action::PruneReviewRefs => self.repository.is_some(),
            Action::CopyPermalink => self.repository.is_some() && self.active.is_some(),
            Action::OpenPullRequestInBrowser
            | Action::PullRequestOverview
            | Action::SubmitReview
            | Action::ApproveReview
            | Action::RequestChanges => self.review_pull().is_some(),
            Action::AddReviewComment => self.can_comment(),
            _ => self.has_threads_here(),
        }
    }

    pub(super) fn pull_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::PullRequests => self.toggle_pull_window(window, cx),
            Action::OpenPullRequestInBrowser => {
                if let Some(pull) = self.review_pull() {
                    cx.open_url(&pull.url);
                }
            }
            Action::AddReviewComment => self.compose_comment(window, cx),
            Action::PullRequestOverview => self.open_overview(window, cx),
            Action::CopyPermalink => self.copy_permalink(cx),
            Action::PruneReviewRefs => self.prune_review_refs(cx),
            Action::SubmitReview | Action::ApproveReview | Action::RequestChanges => {
                self.compose_verdict(action, window, cx);
            }
            Action::SendComment | Action::SendCommentNow | Action::InsertSuggestion => {
                self.compose_action(action, window, cx);
            }
            _ => self.thread_action(action, window, cx),
        }
    }

    fn render_pull_filters(current: PrFilter, cx: &mut Context<Self>) -> gpui::Div {
        let filters = FILTERS.map(|filter| {
            let active = filter == current;
            div()
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(12.))
                .when(active, |tab| tab.bg(theme::ui(cx, UiRole::Selected)))
                .when(!active, |tab| {
                    tab.text_color(theme::ui(cx, UiRole::TextMuted))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.pull_list.filter = filter;
                        this.load_pulls(false, cx);
                    }),
                )
                .child(filter_label(filter))
        });
        div().flex_none().flex().gap(px(2.)).children(filters)
    }

    pub(super) fn render_pull_window(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let list = &self.pull_list;
        let pulls = list.pulls.clone();
        let selected = list.selected;
        let message = match &list.listing {
            Listing::Loading => Some("Loading…".to_owned()),
            Listing::Failed(message) => Some(message.clone()),
            Listing::Loaded if pulls.is_empty() => Some("No open pull requests".to_owned()),
            Listing::Loaded => None,
        };
        let failed = matches!(list.listing, Listing::Failed(_));
        let rows = uniform_list(
            "pull-rows",
            pulls.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| Some((index, pulls.get(index)?)))
                    .map(|(index, pull)| {
                        Self::render_pull_row(index, index == selected, pull, cx).into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&list.scroll)
        .size_full();
        let reviewing = self.review_pull().map(|pull| {
            div()
                .id("pull-reviewing")
                .flex_none()
                .mx(px(8.))
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .truncate()
                .text_color(theme::ui(cx, UiRole::TextMuted))
                .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                .on_click(cx.listener(|this, _, window, cx| this.browse_pulls(false, window, cx)))
                .child(format!("‹ Back to #{} {}", pull.number, pull.title))
        });
        let title = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child(Action::PullRequests.title());
        div()
            .w(super::TOOL_WINDOW_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .children(reviewing)
            .child(Self::render_pull_filters(list.filter, cx).px(px(8.)))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .mx(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.pull_input.clone()),
            )
            .children(message.map(|message| {
                let role = if failed {
                    UiRole::Error
                } else {
                    UiRole::TextFaint
                };
                div()
                    .flex_none()
                    .px(px(12.))
                    .text_color(theme::ui(cx, role))
                    .child(message)
            }))
            .child(div().flex_1().min_h_0().child(rows))
    }

    fn render_pull_row(
        index: usize,
        selected: bool,
        pull: &PullRequestSummary,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(("pull-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex()
            .flex_col()
            .justify_center()
            .px(px(12.))
            .when(selected, |row| row.bg(theme::ui(cx, UiRole::Selected)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.accept_pull(index, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme::ui(cx, UiRole::TextMuted))
                            .child(format!("#{}", pull.number)),
                    )
                    .child(div().truncate().child(pull.title.clone())),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(detail(pull)),
            )
    }
}

#[cfg(test)]
mod tests {
    use dumbj_review::PrState;

    use super::*;

    #[test]
    fn the_filters_cycle_both_ways() {
        let mut filter = PrFilter::ReviewRequested;
        let mut seen = Vec::new();
        for _ in 0..4 {
            filter = next_filter(filter, true);
            seen.push(filter_label(filter));
        }
        assert_eq!(seen, ["Assigned", "Mine", "All", "Review requested"]);
        assert_eq!(next_filter(PrFilter::ReviewRequested, false), PrFilter::All);
    }

    #[test]
    fn a_row_tells_author_branches_day_and_state() {
        let mut pull = PullRequestSummary {
            number: 7,
            title: "Add a playlist summary".to_owned(),
            author: "octo".to_owned(),
            url: String::new(),
            is_draft: true,
            state: PrState::Open,
            updated_at: "2026-11-16T10:00:00Z".to_owned(),
            head_ref: "topic".to_owned(),
            base_ref: "main".to_owned(),
            review_decision: Some(ReviewDecision::ChangesRequested),
        };
        assert_eq!(
            detail(&pull),
            "octo · topic → main · 2026-11-16 · draft · changes requested"
        );
        pull.is_draft = false;
        pull.review_decision = Some(ReviewDecision::ReviewRequired);
        assert_eq!(detail(&pull), "octo · topic → main · 2026-11-16");
    }
}
