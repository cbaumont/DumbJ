use crate::{Client, NewThread, ReviewError, ReviewEvent};

/// A comment to post: a new thread or a reply to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Thread(NewThread),
    Reply { thread: String, body: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingComment {
    pub comment: Outgoing,
    /// Published at once, in a review of its own, instead of joining the pending review.
    /// When a pending review already exists the comment joins it, as GitHub requires.
    pub single: bool,
}

impl Client {
    /// Posts the comments in order and returns each one's outcome. `pending` is the viewer's
    /// pending review; one is started when a comment needs it, and a review started for a
    /// comment that then fails is deleted again.
    pub fn post_comments(
        &self,
        pull_id: &str,
        pending: Option<&str>,
        comments: &[OutgoingComment],
    ) -> Vec<Result<(), ReviewError>> {
        let mut pending = pending.map(str::to_owned);
        comments
            .iter()
            .map(|outgoing| {
                let alone = outgoing.single && pending.is_none();
                let started = pending.is_none();
                let review = match &pending {
                    Some(review) => review.clone(),
                    None => self.start_review(pull_id)?,
                };
                let sent = match &outgoing.comment {
                    Outgoing::Thread(thread) => {
                        self.add_thread(pull_id, Some(&review), thread).map(drop)
                    }
                    Outgoing::Reply { thread, body } => {
                        self.reply(thread, Some(&review), body).map(drop)
                    }
                };
                let sent = sent.and_then(|()| {
                    if alone {
                        self.submit_review(&review, ReviewEvent::Comment, "")
                    } else {
                        Ok(())
                    }
                });
                match &sent {
                    Ok(()) if !alone => pending = Some(review),
                    Err(_) if started => {
                        let _ = self.delete_review(&review);
                    }
                    _ => {}
                }
                sent
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::DiffSide;
    use crate::testing::MockServer;

    const STARTED: &str =
        r#"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R_new"}}}}"#;
    const THREAD: &str = r#"{"data":{"addPullRequestReviewThread":{"thread":{"id":"T","path":"a",
        "diffSide":"RIGHT","line":3,"startLine":null,"originalLine":3,"originalStartLine":null,
        "isResolved":false,"isOutdated":false,"viewerCanResolve":true,"viewerCanUnresolve":false,
        "viewerCanReply":true,"comments":{"nodes":[{"id":"C","body":"b","createdAt":"d","url":"u",
        "author":{"login":"me"},"pullRequestReview":{"state":"PENDING"}}]}}}}}"#;
    const REPLY: &str = r#"{"data":{"addPullRequestReviewThreadReply":{"comment":{"id":"C2",
        "body":"b","createdAt":"d","url":"u","author":{"login":"me"},
        "pullRequestReview":{"state":"PENDING"}}}}}"#;
    const SUBMITTED: &str =
        r#"{"data":{"submitPullRequestReview":{"pullRequestReview":{"id":"R_new"}}}}"#;
    const DELETED: &str =
        r#"{"data":{"deletePullRequestReview":{"pullRequestReview":{"id":"R_new"}}}}"#;
    const REFUSED: &str = r#"{"data":null,"errors":[{"message":"line must be part of the diff"}]}"#;

    fn thread(single: bool) -> OutgoingComment {
        OutgoingComment {
            comment: Outgoing::Thread(NewThread {
                path: "a".to_owned(),
                side: DiffSide::Right,
                line: 3,
                start_line: None,
                body: "b".to_owned(),
            }),
            single,
        }
    }

    fn reply(single: bool) -> OutgoingComment {
        OutgoingComment {
            comment: Outgoing::Reply {
                thread: "T".to_owned(),
                body: "b".to_owned(),
            },
            single,
        }
    }

    /// Posts `comments` against canned responses and returns the outcomes and, for each
    /// request, its mutation name and the review id it carried.
    fn run(
        pending: Option<&str>,
        comments: &[OutgoingComment],
        responses: &[&str],
    ) -> (Vec<bool>, Vec<(String, String)>) {
        let server = MockServer::start(
            responses
                .iter()
                .map(|body| (200, (*body).to_owned()))
                .collect(),
        );
        let client = Client::with_endpoint(server.endpoint(), "t".into());
        let outcomes = client
            .post_comments("PR", pending, comments)
            .iter()
            .map(Result::is_ok)
            .collect();
        let calls = server
            .requests()
            .iter()
            .map(|request| {
                let body: Value = serde_json::from_str(&request.body).unwrap();
                let query = body["query"].as_str().unwrap();
                let name = [
                    "addPullRequestReviewThreadReply",
                    "addPullRequestReviewThread",
                    "addPullRequestReview",
                    "submitPullRequestReview",
                    "deletePullRequestReview",
                ]
                .into_iter()
                .find(|name| query.contains(&format!("{name}(")))
                .unwrap();
                let review = body["variables"]["input"]["pullRequestReviewId"]
                    .as_str()
                    .unwrap_or("-");
                (name.to_owned(), review.to_owned())
            })
            .collect();
        (outcomes, calls)
    }

    fn calls(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, review)| ((*name).to_owned(), (*review).to_owned()))
            .collect()
    }

    #[test]
    fn comments_join_the_pending_review() {
        let (outcomes, sent) = run(Some("R_1"), &[thread(false), reply(true)], &[THREAD, REPLY]);
        assert_eq!(outcomes, [true, true]);
        assert_eq!(
            sent,
            calls(&[
                ("addPullRequestReviewThread", "R_1"),
                ("addPullRequestReviewThreadReply", "R_1"),
            ])
        );
    }

    #[test]
    fn the_first_comment_starts_the_review_the_next_ones_share() {
        let (outcomes, sent) = run(
            None,
            &[thread(false), reply(false)],
            &[STARTED, THREAD, REPLY],
        );
        assert_eq!(outcomes, [true, true]);
        assert_eq!(
            sent,
            calls(&[
                ("addPullRequestReview", "-"),
                ("addPullRequestReviewThread", "R_new"),
                ("addPullRequestReviewThreadReply", "R_new"),
            ])
        );
    }

    #[test]
    fn a_single_comment_is_published_in_its_own_review() {
        let (outcomes, sent) = run(
            None,
            &[thread(true), thread(true)],
            &[STARTED, THREAD, SUBMITTED, STARTED, THREAD, SUBMITTED],
        );
        assert_eq!(outcomes, [true, true]);
        assert_eq!(sent.len(), 6);
        assert_eq!(sent[2], calls(&[("submitPullRequestReview", "R_new")])[0]);
        assert_eq!(sent[3].0, "addPullRequestReview");
    }

    #[test]
    fn a_refused_comment_does_not_leave_an_empty_review_or_stop_the_rest() {
        let (outcomes, sent) = run(
            None,
            &[thread(false), reply(false)],
            &[STARTED, REFUSED, DELETED, STARTED, REPLY],
        );
        assert_eq!(outcomes, [false, true]);
        assert_eq!(
            sent,
            calls(&[
                ("addPullRequestReview", "-"),
                ("addPullRequestReviewThread", "R_new"),
                ("deletePullRequestReview", "R_new"),
                ("addPullRequestReview", "-"),
                ("addPullRequestReviewThreadReply", "R_new"),
            ])
        );
    }

    #[test]
    fn a_refused_comment_keeps_an_existing_pending_review() {
        let (outcomes, sent) = run(Some("R_1"), &[thread(false)], &[REFUSED]);
        assert_eq!(outcomes, [false]);
        assert_eq!(sent, calls(&[("addPullRequestReviewThread", "R_1")]));
    }
}
