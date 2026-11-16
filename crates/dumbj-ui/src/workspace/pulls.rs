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

use super::Workspace;
use crate::theme;

const ROW_HEIGHT: f32 = 26.;
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

pub(super) struct PullPopup {
    filter: PrFilter,
    pulls: Vec<PullRequestSummary>,
    listing: Listing,
    selected: usize,
    scroll: UniformListScrollHandle,
    task: Option<Task<()>>,
}

impl Workspace {
    fn open_pull_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        self.pull_popup = Some(PullPopup {
            filter: PrFilter::ReviewRequested,
            pulls: Vec::new(),
            listing: Listing::Loading,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            task: None,
        });
        self.pull_input
            .update(cx, |input, cx| input.set_text("", false, cx));
        let handle = self.pull_input.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        self.load_pulls(false, cx);
    }

    /// Asks GitHub for the list again, after a pause when the user is typing.
    pub(super) fn load_pulls(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let text = self.pull_input.read(cx).text().to_owned();
        let (Some(repository), Some(popup)) = (self.repository.clone(), self.pull_popup.as_mut())
        else {
            return;
        };
        let (filter, known) = (popup.filter, self.github.clone());
        popup.listing = Listing::Loading;
        popup.task = Some(cx.spawn(async move |this, cx| {
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
                let Some(popup) = this.pull_popup.as_mut() else {
                    return;
                };
                match listed {
                    Ok((github, pulls)) => {
                        this.github = Some(github);
                        popup.pulls = pulls;
                        popup.listing = Listing::Loaded;
                    }
                    Err(message) => {
                        popup.pulls.clear();
                        popup.listing = Listing::Failed(message);
                    }
                }
                popup.selected = 0;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn close_pull_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pull_popup = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    pub(super) fn pull_popup_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(popup) = self.pull_popup.as_mut() else {
            return;
        };
        let last = popup.pulls.len().saturating_sub(1);
        match action {
            Action::MoveUp => popup.selected = popup.selected.saturating_sub(1),
            Action::MoveDown => popup.selected = (popup.selected + 1).min(last),
            Action::MovePageUp => popup.selected = popup.selected.saturating_sub(10),
            Action::MovePageDown => popup.selected = (popup.selected + 10).min(last),
            Action::Tab | Action::Unindent => {
                popup.filter = next_filter(popup.filter, action == Action::Tab);
                self.load_pulls(false, cx);
                return;
            }
            Action::Enter => {
                let selected = popup.selected;
                self.accept_pull(selected, window, cx);
                return;
            }
            Action::Escape => {
                self.close_pull_popup(window, cx);
                return;
            }
            _ => {
                self.pull_input
                    .update(cx, |input, cx| input.handle_action(action, window, cx));
                return;
            }
        }
        popup
            .scroll
            .scroll_to_item(popup.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn accept_pull(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.pull_popup.take() else {
            return;
        };
        self.focus_fallback(window, cx);
        if let Some(pull) = popup.pulls.into_iter().nth(index) {
            self.open_pull(pull, window, cx);
        }
        cx.notify();
    }

    /// Fetches the pull request into `refs/dumbj/pr/` and starts its review.
    fn open_pull(&mut self, pull: PullRequestSummary, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(repository), Some(github)) = (self.repository.clone(), self.github.clone())
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

    pub(super) fn pull_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (action, self.review_pull()) {
            (Action::PullRequests, _) => self.open_pull_popup(window, cx),
            (Action::OpenPullRequestInBrowser, Some(pull)) => cx.open_url(&pull.url),
            (Action::OpenPullRequestInBrowser, None) => {}
            _ => self.thread_action(action, cx),
        }
    }

    fn render_pull_filters(current: PrFilter, cx: &mut Context<Self>) -> gpui::Div {
        let filters = FILTERS.map(|filter| {
            let active = filter == current;
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .when(active, |tab| tab.bg(theme::ui(cx, UiRole::Selected)))
                .when(!active, |tab| {
                    tab.text_color(theme::ui(cx, UiRole::TextMuted))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(popup) = this.pull_popup.as_mut() {
                            popup.filter = filter;
                        }
                        this.load_pulls(false, cx);
                    }),
                )
                .child(filter_label(filter))
        });
        div().flex().gap(px(4.)).children(filters)
    }

    pub(super) fn render_pull_popup(
        &self,
        popup: &PullPopup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pulls = popup.pulls.clone();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a popup shows at most a dozen rows"
        )]
        let list_height = px(ROW_HEIGHT * pulls.len().min(14) as f32);
        let selected = popup.selected;
        let message = match &popup.listing {
            Listing::Loading => Some("Loading…".to_owned()),
            Listing::Failed(message) => Some(message.clone()),
            Listing::Loaded if pulls.is_empty() => Some("No open pull requests".to_owned()),
            Listing::Loaded => None,
        };
        let failed = matches!(popup.listing, Listing::Failed(_));
        let list = uniform_list(
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
        .track_scroll(&popup.scroll)
        .h(list_height);
        let filters = Self::render_pull_filters(popup.filter, cx);
        let body = div()
            .w(px(760.))
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
                    .child(
                        div()
                            .text_color(theme::ui(cx, UiRole::TextMuted))
                            .child(Action::PullRequests.title()),
                    )
                    .child(filters),
            )
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.pull_input.clone()),
            )
            .child(list)
            .children(message.map(|message| {
                let role = if failed {
                    UiRole::Error
                } else {
                    UiRole::TextFaint
                };
                div()
                    .px(px(10.))
                    .text_color(theme::ui(cx, role))
                    .child(message)
            }));
        div()
            .id("pull-popup-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_pull_popup(window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            )
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
                    this.accept_pull(index, window, cx);
                }),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(format!("#{}", pull.number)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(380.))
                    .truncate()
                    .child(pull.title.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
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
