use serde::Deserialize;
use serde_json::json;

use crate::wire::{Author, RepoData, login};
use crate::{Client, PrState, RepoId, ReviewDecision, ReviewError};

const DETAILS_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id number title body url state isDraft headRefName baseRefName headRefOid baseRefOid reviewDecision
      author { login }
      reviews(states: [PENDING], first: 10) { nodes { id viewerDidAuthor } }
      lastCommit: commits(last: 1) {
        nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
          __typename
          ... on CheckRun { name status conclusion detailsUrl }
          ... on StatusContext { context state targetUrl }
        } } } } }
      }
      commits(first: 100) {
        nodes { commit { oid messageHeadline committedDate author { name user { login } } } }
      }
    }
  }
}";

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Success,
    Failure,
    Pending,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullCommit {
    pub oid: String,
    pub headline: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestDetails {
    /// GraphQL node id, needed by the mutations.
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// Login; "ghost" when the account was deleted.
    pub author: String,
    pub state: PrState,
    pub is_draft: bool,
    pub head_ref: String,
    pub base_ref: String,
    pub head_oid: String,
    pub base_oid: String,
    pub review_decision: Option<ReviewDecision>,
    pub pending_review: Option<String>,
    /// Checks of the last commit's status rollup (first 100).
    pub checks: Vec<Check>,
    /// The first 100 commits.
    pub commits: Vec<PullCommit>,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDetails {
    id: String,
    number: u64,
    title: String,
    body: String,
    url: String,
    state: PrState,
    is_draft: bool,
    head_ref_name: String,
    base_ref_name: String,
    head_ref_oid: String,
    base_ref_oid: String,
    review_decision: Option<ReviewDecision>,
    author: Option<Author>,
    reviews: Nodes<RawReview>,
    last_commit: Nodes<LastCommit>,
    commits: Nodes<CommitNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReview {
    id: String,
    viewer_did_author: bool,
}

#[derive(Deserialize)]
struct LastCommit {
    commit: RollupCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollupCommit {
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    contexts: Nodes<Context>,
}

#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum Context {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        #[serde(rename = "detailsUrl")]
        details_url: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
        #[serde(rename = "targetUrl")]
        target_url: Option<String>,
    },
}

impl Context {
    fn into_check(self) -> Check {
        match self {
            Self::CheckRun {
                name,
                status,
                conclusion,
                details_url,
            } => {
                let state = match (status.as_str(), conclusion.as_deref()) {
                    ("COMPLETED", Some("SUCCESS")) => CheckState::Success,
                    (
                        "COMPLETED",
                        Some(
                            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "STARTUP_FAILURE"
                            | "ACTION_REQUIRED",
                        ),
                    ) => CheckState::Failure,
                    ("COMPLETED", Some(_)) => CheckState::Neutral,
                    _ => CheckState::Pending,
                };
                Check {
                    name,
                    state,
                    url: details_url,
                }
            }
            Self::StatusContext {
                context,
                state,
                target_url,
            } => Check {
                name: context,
                state: match state.as_str() {
                    "SUCCESS" => CheckState::Success,
                    "FAILURE" | "ERROR" => CheckState::Failure,
                    _ => CheckState::Pending,
                },
                url: target_url,
            },
        }
    }
}

#[derive(Deserialize)]
struct CommitNode {
    commit: RawCommit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCommit {
    oid: String,
    message_headline: String,
    committed_date: String,
    author: Option<CommitAuthor>,
}

#[derive(Deserialize)]
struct CommitAuthor {
    name: Option<String>,
    user: Option<Author>,
}

impl RawCommit {
    fn into_commit(self) -> PullCommit {
        let author = self
            .author
            .and_then(|a| a.user.map(|u| u.login).or(a.name))
            .unwrap_or_else(|| "ghost".to_string());
        PullCommit {
            oid: self.oid,
            headline: self.message_headline,
            author,
            date: self.committed_date,
        }
    }
}

impl Client {
    pub fn pull_request(
        &self,
        repo: &RepoId,
        number: u64,
    ) -> Result<PullRequestDetails, ReviewError> {
        let data: RepoData<RawDetails> = self.graphql(
            DETAILS_QUERY,
            json!({ "owner": repo.owner, "name": repo.name, "number": number }),
        )?;
        let raw = data.pull_request(number)?;
        let checks = raw
            .last_commit
            .nodes
            .into_iter()
            .next()
            .and_then(|c| c.commit.status_check_rollup)
            .map_or_default(|r| {
                r.contexts
                    .nodes
                    .into_iter()
                    .map(Context::into_check)
                    .collect()
            });
        Ok(PullRequestDetails {
            id: raw.id,
            number: raw.number,
            title: raw.title,
            body: raw.body,
            url: raw.url,
            author: login(raw.author),
            state: raw.state,
            is_draft: raw.is_draft,
            head_ref: raw.head_ref_name,
            base_ref: raw.base_ref_name,
            head_oid: raw.head_ref_oid,
            base_oid: raw.base_ref_oid,
            review_decision: raw.review_decision,
            pending_review: raw
                .reviews
                .nodes
                .into_iter()
                .find(|r| r.viewer_did_author)
                .map(|r| r.id),
            checks,
            commits: raw
                .commits
                .nodes
                .into_iter()
                .map(|n| n.commit.into_commit())
                .collect(),
        })
    }
}
