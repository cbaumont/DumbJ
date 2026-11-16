//! Writing to a pull request: comments, replies and the review verdict. A comment is kept
//! as a draft in the project state before anything is sent, and dropped once GitHub has it.

use std::path::PathBuf;

use dumbj_actions::Action;
use dumbj_review::{DiffSide, NewThread, Outgoing, OutgoingComment, ReviewEvent};
use dumbj_settings::{DraftComment, NoteSide, UiRole};
use gpui::{AppContext, Context, IntoElement, MouseButton, Window, div, prelude::*, px};

use super::Workspace;
use crate::theme;

const EDITOR_HEIGHT: f32 = 220.;

pub(super) enum Purpose {
    Thread {
        path: PathBuf,
        side: NoteSide,
        first_line: usize,
        last_line: usize,
        /// The commented lines, for a suggested change.
        excerpt: String,
        /// The unsent draft being edited.
        draft: Option<u64>,
    },
    Reply {
        thread: String,
        path: PathBuf,
        side: NoteSide,
        line: usize,
    },
    Submit(ReviewEvent),
}

pub(super) struct Compose {
    purpose: Purpose,
}

fn lines_label(first: usize, last: usize) -> String {
    if last > first {
        format!("lines {first}–{last}")
    } else {
        format!("line {first}")
    }
}

impl Compose {
    fn title(&self) -> String {
        match &self.purpose {
            Purpose::Thread {
                path,
                side,
                first_line,
                last_line,
                ..
            } => {
                let side = match side {
                    NoteSide::Base => " (base)",
                    NoteSide::Head => "",
                };
                let lines = lines_label(*first_line, *last_line);
                format!("Comment on {}, {lines}{side}", path.display())
            }
            Purpose::Reply { path, line, .. } => {
                format!("Reply on {}, line {line}", path.display())
            }
            Purpose::Submit(ReviewEvent::Comment) => "Submit review: comment".to_owned(),
            Purpose::Submit(ReviewEvent::Approve) => "Submit review: approve".to_owned(),
            Purpose::Submit(ReviewEvent::RequestChanges) => {
                "Submit review: request changes".to_owned()
            }
        }
    }
}

fn suggestion(excerpt: &str) -> String {
    format!("```suggestion\n{excerpt}\n```\n")
}

fn outgoing(draft: &DraftComment) -> OutgoingComment {
    let line = |line: usize| u32::try_from(line).unwrap_or(u32::MAX);
    let comment = match &draft.thread {
        Some(thread) => Outgoing::Reply {
            thread: thread.clone(),
            body: draft.body.clone(),
        },
        None => Outgoing::Thread(NewThread {
            path: draft.path.to_string_lossy().into_owned(),
            side: match draft.side {
                NoteSide::Base => DiffSide::Left,
                NoteSide::Head => DiffSide::Right,
            },
            line: line(draft.last_line),
            start_line: (draft.first_line < draft.last_line).then(|| line(draft.first_line)),
            body: draft.body.clone(),
        }),
    };
    OutgoingComment {
        comment,
        single: draft.single,
    }
}

impl Workspace {
    pub(super) fn compose_open(&self) -> bool {
        self.compose.is_some()
    }

    fn open_compose(
        &mut self,
        purpose: Purpose,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.compose = Some(Compose { purpose });
        self.compose_editor
            .update(cx, |editor, cx| editor.set_scratch_text(text, cx));
        let handle = self.compose_editor.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn close_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compose = None;
        self.focus_fallback(window, cx);
        cx.notify();
    }

    pub(super) fn can_comment(&self) -> bool {
        self.review_pull().is_some() && self.note_site().is_some()
    }

    /// Opens the comment field for the selected lines of the diff side the caret is in.
    pub(super) fn compose_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_pull().is_none() {
            return;
        }
        let Some((path, side, editor)) = self.note_site() else {
            return;
        };
        let editor = editor.read(cx);
        let (first_line, last_line) = editor.selected_lines();
        let excerpt = editor.lines_text(first_line, last_line);
        let purpose = Purpose::Thread {
            path,
            side,
            first_line,
            last_line,
            excerpt,
            draft: None,
        };
        self.open_compose(purpose, "", window, cx);
    }

    pub(super) fn compose_reply(
        &mut self,
        thread: String,
        path: PathBuf,
        side: NoteSide,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let purpose = Purpose::Reply {
            thread,
            path,
            side,
            line,
        };
        self.open_compose(purpose, "", window, cx);
    }

    /// Reopens a comment that was not sent, to change it, send it again or drop it.
    pub(super) fn compose_draft(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self
            .state
            .review
            .as_ref()
            .and_then(|review| review.drafts().iter().find(|draft| draft.id == id))
            .cloned();
        let Some(draft) = draft else { return };
        let purpose = match draft.thread {
            Some(thread) => Purpose::Reply {
                thread,
                path: draft.path,
                side: draft.side,
                line: draft.last_line,
            },
            None => Purpose::Thread {
                path: draft.path,
                side: draft.side,
                first_line: draft.first_line,
                last_line: draft.last_line,
                excerpt: String::new(),
                draft: Some(id),
            },
        };
        if let Some(review) = self.state.review.as_mut()
            && matches!(purpose, Purpose::Reply { .. })
        {
            review.remove_draft(id);
        }
        self.open_compose(purpose, &draft.body, window, cx);
    }

    pub(super) fn compose_verdict(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let event = match action {
            Action::ApproveReview => ReviewEvent::Approve,
            Action::RequestChanges => ReviewEvent::RequestChanges,
            _ => ReviewEvent::Comment,
        };
        self.open_compose(Purpose::Submit(event), "", window, cx);
    }

    pub(super) fn compose_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::Escape => self.close_compose(window, cx),
            Action::SendComment | Action::SendCommentNow => {
                self.accept_compose(action == Action::SendCommentNow, window, cx);
            }
            Action::InsertSuggestion => {
                let excerpt = match self.compose.as_ref().map(|compose| &compose.purpose) {
                    Some(Purpose::Thread { excerpt, .. }) if !excerpt.is_empty() => excerpt.clone(),
                    _ => return,
                };
                self.compose_editor.update(cx, |editor, cx| {
                    let mut text = editor.text();
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&suggestion(&excerpt));
                    editor.set_scratch_text(&text, cx);
                });
            }
            _ => {
                self.compose_editor
                    .update(cx, |editor, cx| editor.handle_action(action, cx));
            }
        }
    }

    fn accept_compose(&mut self, single: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose) = self.compose.take() else {
            return;
        };
        let body = self.compose_editor.read(cx).text().trim().to_owned();
        self.focus_fallback(window, cx);
        let draft = match compose.purpose {
            Purpose::Submit(event) => {
                self.submit_review(event, body, cx);
                return;
            }
            Purpose::Thread {
                path,
                side,
                first_line,
                last_line,
                draft,
                ..
            } => {
                if let (Some(id), Some(review)) = (draft, self.state.review.as_mut()) {
                    review.remove_draft(id);
                }
                DraftComment {
                    id: 0,
                    path,
                    side,
                    first_line,
                    last_line,
                    body,
                    thread: None,
                    single,
                }
            }
            Purpose::Reply {
                thread,
                path,
                side,
                line,
            } => DraftComment {
                id: 0,
                path,
                side,
                first_line: line,
                last_line: line,
                body,
                thread: Some(thread),
                single,
            },
        };
        if let Some(review) = self.state.review.as_mut()
            && !draft.body.is_empty()
        {
            review.add_draft(draft);
        }
        self.save_project_state(cx);
        self.apply_thread_blocks(cx);
        self.send_drafts(cx);
        cx.notify();
    }

    /// Sends every unsent comment. Without the pull request's details it reads them first,
    /// which calls this again.
    pub(super) fn send_drafts(&mut self, cx: &mut Context<Self>) {
        let drafts = self
            .state
            .review
            .as_ref()
            .map_or_else(Vec::new, |review| review.drafts().to_vec());
        if drafts.is_empty() || self.session.sending {
            return;
        }
        let ready = self.session.github.clone().zip(
            self.session
                .pull_threads
                .as_ref()
                .and_then(|pull| pull.details.clone()),
        );
        let Some((github, details)) = ready else {
            self.load_pull_threads(cx);
            return;
        };
        self.session.sending = true;
        self.status_message = Some("Sending…".to_owned());
        cx.spawn(async move |this, cx| {
            let outcomes = cx
                .background_spawn(async move {
                    let comments: Vec<OutgoingComment> = drafts.iter().map(outgoing).collect();
                    let sent = github.client.post_comments(
                        &details.id,
                        details.pending_review.as_deref(),
                        &comments,
                    );
                    drafts
                        .iter()
                        .map(|draft| draft.id)
                        .zip(sent.into_iter().map(|sent| sent.map_err(|e| e.to_string())))
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.session.sending = false;
                let mut failure = None;
                for (id, outcome) in outcomes {
                    match outcome {
                        Ok(()) => {
                            if let Some(review) = this.state.review.as_mut() {
                                review.remove_draft(id);
                            }
                        }
                        Err(message) => failure = Some(message),
                    }
                }
                this.save_project_state(cx);
                this.status_message = Some(failure.map_or_else(
                    || "Sent".to_owned(),
                    |message| format!("Not sent, kept as a draft: {message}"),
                ));
                this.reload_pull_threads(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn submit_review(&mut self, event: ReviewEvent, body: String, cx: &mut Context<Self>) {
        let ready = self.session.github.clone().zip(
            self.session
                .pull_threads
                .as_ref()
                .and_then(|pull| pull.details.clone()),
        );
        let Some((github, details)) = ready else {
            self.status_message = Some("The pull request is not loaded yet".to_owned());
            return;
        };
        self.status_message = Some("Submitting the review…".to_owned());
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    match &details.pending_review {
                        Some(review) => github.client.submit_review(review, event, &body),
                        None => github.client.add_review(&details.id, event, &body),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.status_message = Some(match outcome {
                    Ok(()) => "Review submitted".to_owned(),
                    Err(error) => format!("Review not submitted: {error}"),
                });
                this.reload_pull_threads(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_compose(
        &self,
        compose: &Compose,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let keys = |action: Action| self.shortcut(action).unwrap_or_default();
        let hint = match &compose.purpose {
            Purpose::Submit(_) => format!("{} submits", keys(Action::SendComment)),
            Purpose::Thread { .. } => format!(
                "{} adds to the review · {} sends now · {} suggests a change",
                keys(Action::SendComment),
                keys(Action::SendCommentNow),
                keys(Action::InsertSuggestion)
            ),
            Purpose::Reply { .. } => format!(
                "{} adds to the review · {} sends now",
                keys(Action::SendComment),
                keys(Action::SendCommentNow)
            ),
        };
        let popup = div()
            .w(px(760.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_color(theme::ui(cx, UiRole::Text))
            .child(
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(compose.title()),
            )
            .child(
                div()
                    .h(px(EDITOR_HEIGHT))
                    .rounded(px(4.))
                    .overflow_hidden()
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .child(self.compose_editor.clone()),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint))
                    .child(hint),
            );
        div()
            .id("compose-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_compose(window, cx)),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(popup),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(first: usize, last: usize, side: NoteSide) -> DraftComment {
        DraftComment {
            id: 1,
            path: PathBuf::from("src/A.kt"),
            side,
            first_line: first,
            last_line: last,
            body: "text".to_owned(),
            thread: None,
            single: false,
        }
    }

    #[test]
    fn a_draft_becomes_a_thread_on_its_last_line_or_a_reply() {
        let Outgoing::Thread(thread) = outgoing(&draft(3, 5, NoteSide::Head)).comment else {
            unreachable!("a draft without a thread starts one")
        };
        assert_eq!(
            (
                thread.path.as_str(),
                thread.side,
                thread.line,
                thread.start_line
            ),
            ("src/A.kt", DiffSide::Right, 5, Some(3))
        );
        let Outgoing::Thread(thread) = outgoing(&draft(4, 4, NoteSide::Base)).comment else {
            unreachable!("a draft without a thread starts one")
        };
        assert_eq!(
            (thread.side, thread.line, thread.start_line),
            (DiffSide::Left, 4, None)
        );
        let mut reply = draft(4, 4, NoteSide::Head);
        reply.thread = Some("T".to_owned());
        reply.single = true;
        let sent = outgoing(&reply);
        assert!(sent.single);
        assert_eq!(
            sent.comment,
            Outgoing::Reply {
                thread: "T".to_owned(),
                body: "text".to_owned()
            }
        );
    }

    #[test]
    fn titles_and_suggestions() {
        let compose = Compose {
            purpose: Purpose::Thread {
                path: PathBuf::from("src/A.kt"),
                side: NoteSide::Base,
                first_line: 2,
                last_line: 4,
                excerpt: String::new(),
                draft: None,
            },
        };
        assert_eq!(compose.title(), "Comment on src/A.kt, lines 2–4 (base)");
        assert_eq!(
            suggestion("val a = 1\nval b = 2"),
            "```suggestion\nval a = 1\nval b = 2\n```\n"
        );
    }
}
