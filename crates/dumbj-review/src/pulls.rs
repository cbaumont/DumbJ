use serde::Deserialize;
use serde_json::json;

use crate::{Client, RepoId, ReviewError};

const PULL_REQUESTS_QUERY: &str = "query($q: String!) {
  search(query: $q, type: ISSUE, first: 50) {
    nodes {
      ... on PullRequest {
        number title url isDraft state updatedAt headRefName baseRefName reviewDecision
        author { login }
      }
    }
  }
}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrFilter {
    ReviewRequested,
    Assigned,
    Authored,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestSummary {
    pub number: u64,
    pub title: String,
    /// Login; "ghost" when the account was deleted.
    pub author: String,
    pub url: String,
    pub is_draft: bool,
    pub state: PrState,
    pub updated_at: String,
    pub head_ref: String,
    pub base_ref: String,
    pub review_decision: Option<ReviewDecision>,
}

#[derive(Deserialize)]
struct SearchData {
    search: Nodes,
}

#[derive(Deserialize)]
struct Nodes {
    nodes: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    number: u64,
    title: String,
    url: String,
    is_draft: bool,
    state: PrState,
    updated_at: String,
    head_ref_name: String,
    base_ref_name: String,
    review_decision: Option<ReviewDecision>,
    author: Option<Author>,
}

#[derive(Deserialize)]
struct Author {
    login: String,
}

fn search_string(repo: &RepoId, filter: PrFilter, text: &str) -> String {
    let mut q = format!(
        "repo:{}/{} is:pr is:open sort:updated-desc",
        repo.owner, repo.name
    );
    match filter {
        PrFilter::ReviewRequested => q.push_str(" review-requested:@me"),
        PrFilter::Assigned => q.push_str(" assignee:@me"),
        PrFilter::Authored => q.push_str(" author:@me"),
        PrFilter::All => {}
    }
    let text = text.trim();
    if !text.is_empty() {
        q.push(' ');
        q.push_str(text);
    }
    q
}

impl Client {
    pub fn pull_requests(
        &self,
        repo: &RepoId,
        filter: PrFilter,
        text: &str,
    ) -> Result<Vec<PullRequestSummary>, ReviewError> {
        let data: SearchData = self.graphql(
            PULL_REQUESTS_QUERY,
            json!({ "q": search_string(repo, filter, text) }),
        )?;
        data.search
            .nodes
            .into_iter()
            .filter(|n| n.as_object().is_none_or(|o| !o.is_empty()))
            .map(|n| {
                let n: Node = serde_json::from_value(n)?;
                Ok(PullRequestSummary {
                    number: n.number,
                    title: n.title,
                    author: n.author.map_or_else(|| "ghost".to_string(), |a| a.login),
                    url: n.url,
                    is_draft: n.is_draft,
                    state: n.state,
                    updated_at: n.updated_at,
                    head_ref: n.head_ref_name,
                    base_ref: n.base_ref_name,
                    review_decision: n.review_decision,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockServer;

    fn repo() -> RepoId {
        RepoId {
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        }
    }

    #[test]
    fn search_strings() {
        let base = "repo:o/r is:pr is:open sort:updated-desc";
        assert_eq!(search_string(&repo(), PrFilter::All, ""), base);
        assert_eq!(
            search_string(&repo(), PrFilter::ReviewRequested, ""),
            format!("{base} review-requested:@me")
        );
        assert_eq!(
            search_string(&repo(), PrFilter::Assigned, ""),
            format!("{base} assignee:@me")
        );
        assert_eq!(
            search_string(&repo(), PrFilter::Authored, " fix bug \n"),
            format!("{base} author:@me fix bug")
        );
        assert_eq!(
            search_string(&repo(), PrFilter::All, "label:x"),
            format!("{base} label:x")
        );
    }

    const LIST: &str = r#"{"data":{"search":{"nodes":[
      {"number":3,"title":"Add thing","url":"https://github.com/o/r/pull/3","isDraft":false,
       "state":"OPEN","updatedAt":"2026-11-16T10:00:00Z","headRefName":"feat","baseRefName":"main",
       "reviewDecision":"APPROVED","author":{"login":"alice"}},
      {"number":2,"title":"Draft","url":"u2","isDraft":true,"state":"OPEN",
       "updatedAt":"2026-11-15T10:00:00Z","headRefName":"d","baseRefName":"main",
       "reviewDecision":"CHANGES_REQUESTED","author":null},
      {"number":1,"title":"Old","url":"u1","isDraft":false,"state":"MERGED",
       "updatedAt":"2026-11-14T10:00:00Z","headRefName":"o","baseRefName":"dev",
       "reviewDecision":"REVIEW_REQUIRED","author":{"login":"bob"}},
      {"number":0,"title":"Closed","url":"u0","isDraft":false,"state":"CLOSED",
       "updatedAt":"2026-11-13T10:00:00Z","headRefName":"c","baseRefName":"main",
       "reviewDecision":null,"author":{"login":"carol"}},
      {}
    ]}}}"#;

    #[test]
    fn parses_pull_requests_and_sends_token_and_variables() {
        let server = MockServer::start(vec![(200, LIST.to_string())]);
        let client = Client::with_endpoint(server.endpoint(), "tok123".into());
        let prs = client
            .pull_requests(&repo(), PrFilter::ReviewRequested, "fix")
            .unwrap();
        assert_eq!(prs.len(), 4);
        assert_eq!(prs[0].number, 3);
        assert_eq!(prs[0].author, "alice");
        assert_eq!(prs[0].review_decision, Some(ReviewDecision::Approved));
        assert_eq!(prs[0].head_ref, "feat");
        assert_eq!(prs[0].base_ref, "main");
        assert!(prs[1].is_draft);
        assert_eq!(prs[1].author, "ghost");
        assert_eq!(
            prs[1].review_decision,
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(prs[2].state, PrState::Merged);
        assert_eq!(prs[2].review_decision, Some(ReviewDecision::ReviewRequired));
        assert_eq!(prs[3].state, PrState::Closed);
        assert_eq!(prs[3].review_decision, None);

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/graphql");
        assert_eq!(requests[0].authorization.as_deref(), Some("bearer tok123"));
        let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(
            body["variables"]["q"],
            "repo:o/r is:pr is:open sort:updated-desc review-requested:@me fix"
        );
        assert!(body["query"].as_str().unwrap().contains("search("));
    }

    fn fail(status: u16, body: &str) -> ReviewError {
        let server = MockServer::start(vec![(status, body.to_string())]);
        Client::with_endpoint(server.endpoint(), "t".into())
            .pull_requests(&repo(), PrFilter::All, "")
            .unwrap_err()
    }

    #[test]
    fn maps_failures() {
        assert!(matches!(
            fail(401, r#"{"message":"Bad credentials"}"#),
            ReviewError::Unauthorized
        ));
        assert!(matches!(fail(500, "boom"), ReviewError::Http(m) if m.contains("500")));
        assert!(matches!(
            fail(200, r#"{"data":{"search":{"nodes":[]}},"errors":[{"message":"a"},{"message":"b"}]}"#),
            ReviewError::Api(m) if m == "a; b"
        ));
        assert!(matches!(fail(200, "not json"), ReviewError::Decode(_)));
        assert!(matches!(
            fail(200, r#"{"data":{"x":1}}"#),
            ReviewError::Decode(_)
        ));
    }

    #[test]
    fn connection_refused_is_http() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let client = Client::with_endpoint(format!("http://127.0.0.1:{port}/graphql"), "t".into());
        assert!(matches!(
            client.pull_requests(&repo(), PrFilter::All, ""),
            Err(ReviewError::Http(_))
        ));
    }

    #[test]
    fn debug_hides_the_token() {
        let client = Client::new("github.com", "secret".into());
        let text = format!("{client:?}");
        assert!(!text.contains("secret"));
        assert!(text.contains("https://api.github.com/graphql"));
        let ghe = Client::new("ghe.corp", "secret".into());
        assert!(format!("{ghe:?}").contains("https://ghe.corp/api/graphql"));
    }
}
