use serde_json::json;

use crate::testing::{client, repo};
use crate::{CheckState, PrState, ReviewDecision, ReviewError};

fn details(rollup: &str, reviews: &str) -> String {
    format!(
        r#"{{"data":{{"repository":{{"pullRequest":{{
        "id":"PR_1","number":7,"title":"Add","body":"text","url":"https://github.com/o/r/pull/7",
        "state":"OPEN","isDraft":true,"headRefName":"feat","baseRefName":"main",
        "headRefOid":"hhh","baseRefOid":"bbb","reviewDecision":"REVIEW_REQUIRED",
        "author":null,
        "reviews":{{"nodes":{reviews}}},
        "lastCommit":{{"nodes":[{{"commit":{{"statusCheckRollup":{rollup}}}}}]}},
        "commits":{{"nodes":[
          {{"commit":{{"oid":"c1","messageHeadline":"First","committedDate":"2026-11-16T12:00:00Z",
            "author":{{"name":"Alice A","user":{{"login":"alice"}}}}}}}},
          {{"commit":{{"oid":"c2","messageHeadline":"Second","committedDate":"2026-11-16T13:00:00Z",
            "author":{{"name":"Bob B","user":null}}}}}},
          {{"commit":{{"oid":"c3","messageHeadline":"Third","committedDate":"2026-11-16T14:00:00Z",
            "author":null}}}}
        ]}}
        }}}}}}}}"#
    )
}

const ROLLUP: &str = r#"{"contexts":{"nodes":[
  {"__typename":"CheckRun","name":"build","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://ci/1"},
  {"__typename":"CheckRun","name":"lint","status":"COMPLETED","conclusion":"TIMED_OUT","detailsUrl":null},
  {"__typename":"CheckRun","name":"docs","status":"COMPLETED","conclusion":"SKIPPED","detailsUrl":"https://ci/3"},
  {"__typename":"CheckRun","name":"slow","status":"IN_PROGRESS","conclusion":null,"detailsUrl":"https://ci/4"},
  {"__typename":"StatusContext","context":"legacy/ok","state":"SUCCESS","targetUrl":"https://legacy/1"},
  {"__typename":"StatusContext","context":"legacy/bad","state":"ERROR","targetUrl":null},
  {"__typename":"StatusContext","context":"legacy/wait","state":"EXPECTED","targetUrl":null}
]}}"#;

#[test]
fn decodes_details_checks_and_commits() {
    let reviews =
        r#"[{"id":"R_other","viewerDidAuthor":false},{"id":"R_mine","viewerDidAuthor":true}]"#;
    let (server, client) = client(vec![details(ROLLUP, reviews)]);
    let pr = client.pull_request(&repo(), 7).unwrap();
    assert_eq!(pr.id, "PR_1");
    assert_eq!(pr.number, 7);
    assert_eq!(pr.author, "ghost");
    assert_eq!(pr.state, PrState::Open);
    assert!(pr.is_draft);
    assert_eq!(
        (pr.head_ref.as_str(), pr.base_ref.as_str()),
        ("feat", "main")
    );
    assert_eq!((pr.head_oid.as_str(), pr.base_oid.as_str()), ("hhh", "bbb"));
    assert_eq!(pr.review_decision, Some(ReviewDecision::ReviewRequired));
    assert_eq!(pr.pending_review.as_deref(), Some("R_mine"));
    let checks: Vec<_> = pr
        .checks
        .iter()
        .map(|c| (c.name.as_str(), c.state, c.url.as_deref()))
        .collect();
    assert_eq!(
        checks,
        vec![
            ("build", CheckState::Success, Some("https://ci/1")),
            ("lint", CheckState::Failure, None),
            ("docs", CheckState::Neutral, Some("https://ci/3")),
            ("slow", CheckState::Pending, Some("https://ci/4")),
            ("legacy/ok", CheckState::Success, Some("https://legacy/1")),
            ("legacy/bad", CheckState::Failure, None),
            ("legacy/wait", CheckState::Pending, None),
        ]
    );
    let authors: Vec<_> = pr.commits.iter().map(|c| c.author.as_str()).collect();
    assert_eq!(authors, ["alice", "Bob B", "ghost"]);
    assert_eq!(pr.commits[0].oid, "c1");
    assert_eq!(pr.commits[0].headline, "First");
    assert_eq!(pr.commits[0].date, "2026-11-16T12:00:00Z");

    let requests = server.requests();
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(
        body["variables"],
        json!({ "owner": "o", "name": "r", "number": 7 })
    );
    assert!(
        body["query"]
            .as_str()
            .unwrap()
            .contains("statusCheckRollup")
    );
}

#[test]
fn no_rollup_and_no_pending_review() {
    let (_server, client) = client(vec![details("null", "[]")]);
    let pr = client.pull_request(&repo(), 7).unwrap();
    assert_eq!(pr.checks.len(), 0);
    assert_eq!(pr.pending_review, None);
}

#[test]
fn missing_pull_request_is_a_decode_error() {
    let (_server, client) = client(vec![r#"{"data":{"repository":{"pullRequest":null}}}"#]);
    assert!(matches!(
        client.pull_request(&repo(), 9),
        Err(ReviewError::Decode(m)) if m.contains("#9")
    ));
}
