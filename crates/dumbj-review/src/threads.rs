use serde::Deserialize;
use serde_json::json;

use crate::wire::{Author, PageInfo, RepoData, login, missing};
use crate::{Client, RepoId, ReviewError};

macro_rules! thread_fields {
    () => {
        "id path diffSide line startLine originalLine originalStartLine
        isResolved isOutdated viewerCanResolve viewerCanUnresolve viewerCanReply
        comments(first: 100) { nodes { id body createdAt url author { login } pullRequestReview { state } } }"
    };
}

macro_rules! comment_fields {
    () => {
        "id body createdAt url author { login } pullRequestReview { state }"
    };
}

const THREADS_QUERY: &str = concat!(
    "query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { ",
    thread_fields!(),
    " }
      }
    }
  }
}"
);

const ADD_THREAD_MUTATION: &str = concat!(
    "mutation($input: AddPullRequestReviewThreadInput!) {
  addPullRequestReviewThread(input: $input) { thread { ",
    thread_fields!(),
    " } }
}"
);

const REPLY_MUTATION: &str = concat!(
    "mutation($input: AddPullRequestReviewThreadReplyInput!) {
  addPullRequestReviewThreadReply(input: $input) { comment { ",
    comment_fields!(),
    " } }
}"
);

const RESOLVE_MUTATION: &str = "mutation($input: ResolveReviewThreadInput!) {
  resolveReviewThread(input: $input) { thread { id } }
}";

const UNRESOLVE_MUTATION: &str = "mutation($input: UnresolveReviewThreadInput!) {
  unresolveReviewThread(input: $input) { thread { id } }
}";

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiffSide {
    Left,
    Right,
}

impl DiffSide {
    fn as_graphql(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    pub id: String,
    /// Login; "ghost" when the account was deleted.
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub pending: bool,
}

#[expect(clippy::struct_excessive_bools, reason = "mirrors the GraphQL flags")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    pub side: DiffSide,
    /// Current position; `None` when the thread is outdated.
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    pub original_line: Option<u32>,
    pub original_start_line: Option<u32>,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub can_resolve: bool,
    pub can_unresolve: bool,
    pub can_reply: bool,
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub path: String,
    pub side: DiffSide,
    pub line: u32,
    pub start_line: Option<u32>,
    pub body: String,
}

#[derive(Deserialize)]
struct ReviewState {
    state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawComment {
    id: String,
    body: String,
    created_at: String,
    url: String,
    author: Option<Author>,
    pull_request_review: Option<ReviewState>,
}

impl RawComment {
    fn into_comment(self) -> ReviewComment {
        ReviewComment {
            id: self.id,
            author: login(self.author),
            body: self.body,
            created_at: self.created_at,
            url: self.url,
            pending: self
                .pull_request_review
                .is_some_and(|r| r.state == "PENDING"),
        }
    }
}

#[derive(Deserialize)]
struct CommentNodes {
    nodes: Vec<RawComment>,
}

#[expect(clippy::struct_excessive_bools, reason = "mirrors the GraphQL flags")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThread {
    id: String,
    path: String,
    diff_side: DiffSide,
    line: Option<u32>,
    start_line: Option<u32>,
    original_line: Option<u32>,
    original_start_line: Option<u32>,
    is_resolved: bool,
    is_outdated: bool,
    viewer_can_resolve: bool,
    viewer_can_unresolve: bool,
    viewer_can_reply: bool,
    comments: CommentNodes,
}

impl RawThread {
    fn into_thread(self) -> Result<ReviewThread, ReviewError> {
        if self.comments.nodes.is_empty() {
            return Err(ReviewError::Decode(format!(
                "review thread {} has no comments",
                self.id
            )));
        }
        Ok(ReviewThread {
            id: self.id,
            path: self.path,
            side: self.diff_side,
            line: self.line,
            start_line: self.start_line,
            original_line: self.original_line,
            original_start_line: self.original_start_line,
            is_resolved: self.is_resolved,
            is_outdated: self.is_outdated,
            can_resolve: self.viewer_can_resolve,
            can_unresolve: self.viewer_can_unresolve,
            can_reply: self.viewer_can_reply,
            comments: self
                .comments
                .nodes
                .into_iter()
                .map(RawComment::into_comment)
                .collect(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadPage {
    review_threads: ThreadConnection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadConnection {
    page_info: PageInfo,
    nodes: Vec<RawThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddThreadData {
    add_pull_request_review_thread: Option<AddThreadPayload>,
}

#[derive(Deserialize)]
struct AddThreadPayload {
    thread: Option<RawThread>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplyData {
    add_pull_request_review_thread_reply: Option<ReplyPayload>,
}

#[derive(Deserialize)]
struct ReplyPayload {
    comment: Option<RawComment>,
}

impl Client {
    /// All review threads, following pagination (100 per page; the first 100 comments of each thread).
    pub fn review_threads(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<Vec<ReviewThread>, ReviewError> {
        let mut threads = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let data: RepoData<ThreadPage> = self.graphql(
                THREADS_QUERY,
                json!({ "owner": repo.owner, "name": repo.name, "number": number, "after": after }),
            )?;
            let page = data.pull_request(number)?.review_threads;
            for raw in page.nodes {
                threads.push(raw.into_thread()?);
            }
            match page.page_info.next()? {
                Some(cursor) => after = Some(cursor),
                None => return Ok(threads),
            }
        }
    }

    /// Creates a thread. With `review_id` it joins that pending review; without one GitHub is
    /// expected to attach it to the viewer's pending review, creating one (not verified).
    pub fn add_thread(
        &self,
        pull_id: &str,
        review_id: Option<&str>,
        thread: &NewThread,
    ) -> Result<ReviewThread, ReviewError> {
        let mut input = json!({
            "pullRequestId": pull_id,
            "path": thread.path,
            "body": thread.body,
            "line": thread.line,
            "side": thread.side.as_graphql(),
        });
        if let Some(start) = thread.start_line {
            input["startLine"] = json!(start);
            input["startSide"] = json!(thread.side.as_graphql());
        }
        if let Some(review_id) = review_id {
            input["pullRequestReviewId"] = json!(review_id);
        }
        let data: AddThreadData = self.graphql(ADD_THREAD_MUTATION, json!({ "input": input }))?;
        data.add_pull_request_review_thread
            .and_then(|p| p.thread)
            .ok_or_else(|| missing("thread"))?
            .into_thread()
    }

    pub fn reply(
        &self,
        thread_id: &str,
        review_id: Option<&str>,
        body: &str,
    ) -> Result<ReviewComment, ReviewError> {
        let mut input = json!({ "pullRequestReviewThreadId": thread_id, "body": body });
        if let Some(review_id) = review_id {
            input["pullRequestReviewId"] = json!(review_id);
        }
        let data: ReplyData = self.graphql(REPLY_MUTATION, json!({ "input": input }))?;
        data.add_pull_request_review_thread_reply
            .and_then(|p| p.comment)
            .map(RawComment::into_comment)
            .ok_or_else(|| missing("comment"))
    }

    pub fn resolve_thread(&self, thread_id: &str) -> Result<(), ReviewError> {
        self.mutate(
            RESOLVE_MUTATION,
            json!({ "input": { "threadId": thread_id } }),
        )
    }

    pub fn unresolve_thread(&self, thread_id: &str) -> Result<(), ReviewError> {
        self.mutate(
            UNRESOLVE_MUTATION,
            json!({ "input": { "threadId": thread_id } }),
        )
    }
}
