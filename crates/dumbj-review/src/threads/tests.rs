use serde_json::{Value, json};

use crate::testing::MockServer;
use crate::{Client, DiffSide, NewThread, RepoId, ReviewError};

fn repo() -> RepoId {
    RepoId {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
    }
}

fn client(responses: Vec<String>) -> (MockServer, Client) {
    let server = MockServer::start(responses.into_iter().map(|b| (200, b)).collect());
    let client = Client::with_endpoint(server.endpoint(), "t".into());
    (server, client)
}

fn variables(server: &MockServer, index: usize) -> Value {
    let body: Value = serde_json::from_str(&server.requests()[index].body).unwrap();
    body["variables"].clone()
}

fn thread_json(id: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"{id}","path":"src/A.kt","diffSide":"RIGHT","line":12,"startLine":null,
        "originalLine":12,"originalStartLine":null,"isResolved":false,"isOutdated":false,
        "viewerCanResolve":true,"viewerCanUnresolve":false,"viewerCanReply":true,
        "comments":{{"nodes":[{{"id":"C_{id}","body":"Why?","createdAt":"2026-11-16T10:00:00Z",
        "url":"https://github.com/o/r/pull/1#discussion_r1","author":{{"login":"alice"}},
        "pullRequestReview":{{"state":"COMMENTED"}}}}]}}{extra}}}"#
    )
}

fn page(threads: &[String], next: Option<&str>) -> String {
    let (has, cursor) = next.map_or(("false", "null".to_string()), |c| {
        ("true", format!("\"{c}\""))
    });
    format!(
        r#"{{"data":{{"repository":{{"pullRequest":{{"reviewThreads":{{
        "pageInfo":{{"hasNextPage":{has},"endCursor":{cursor}}},"nodes":[{}]}}}}}}}}}}"#,
        threads.join(",")
    )
}

#[test]
fn follows_thread_pagination() {
    let (server, client) = client(vec![
        page(&[thread_json("T1", "")], Some("cur1")),
        page(&[thread_json("T2", "")], None),
    ]);
    let threads = client.review_threads(&repo(), 1).unwrap();
    let ids: Vec<_> = threads.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["T1", "T2"]);
    assert_eq!(server.requests().len(), 2);
    assert_eq!(
        variables(&server, 0),
        json!({ "owner": "o", "name": "r", "number": 1, "after": null })
    );
    assert_eq!(variables(&server, 1)["after"], "cur1");
    let t = &threads[0];
    assert_eq!(t.path, "src/A.kt");
    assert_eq!(t.side, DiffSide::Right);
    assert_eq!(t.line, Some(12));
    assert_eq!(t.start_line, None);
    assert!(t.can_resolve && t.can_reply && !t.can_unresolve);
    assert_eq!(t.comments[0].author, "alice");
    assert!(!t.comments[0].pending);
}

#[test]
fn outdated_thread_has_no_current_line() {
    let outdated = r#"{"id":"T9","path":"B.kt","diffSide":"LEFT","line":null,"startLine":null,
        "originalLine":5,"originalStartLine":3,"isResolved":true,"isOutdated":true,
        "viewerCanResolve":false,"viewerCanUnresolve":true,"viewerCanReply":false,
        "comments":{"nodes":[{"id":"C9","body":"old","createdAt":"d","url":"u","author":null,
        "pullRequestReview":null}]}}"#;
    let (_server, client) = client(vec![page(&[outdated.to_string()], None)]);
    let threads = client.review_threads(&repo(), 1).unwrap();
    let t = &threads[0];
    assert_eq!(t.side, DiffSide::Left);
    assert_eq!(t.line, None);
    assert_eq!(t.original_line, Some(5));
    assert_eq!(t.original_start_line, Some(3));
    assert!(t.is_resolved && t.is_outdated && t.can_unresolve);
    assert_eq!(t.comments[0].author, "ghost");
}

#[test]
fn pending_comment_is_flagged() {
    let pending = thread_json("T1", "").replace("COMMENTED", "PENDING");
    let (_server, client) = client(vec![page(&[pending], None)]);
    let threads = client.review_threads(&repo(), 1).unwrap();
    assert!(threads[0].comments[0].pending);
}

#[test]
fn thread_without_comments_is_a_decode_error() {
    let empty = thread_json("T1", "").replace(
        r#""comments":{"nodes":[{"id":"C_T1""#,
        r#""comments":{"nodes":[],"x":[{"id":"C_T1""#,
    );
    let (_server, client) = client(vec![page(&[empty], None)]);
    assert!(matches!(
        client.review_threads(&repo(), 1),
        Err(ReviewError::Decode(m)) if m.contains("T1")
    ));
}

fn add_response(thread: &str) -> String {
    format!(r#"{{"data":{{"addPullRequestReviewThread":{{"thread":{thread}}}}}}}"#)
}

#[test]
fn add_thread_sends_the_input() {
    let (server, client) = client(vec![
        add_response(&thread_json("T5", "")),
        add_response(&thread_json("T6", "")),
    ]);
    let single = NewThread {
        path: "src/A.kt".into(),
        side: DiffSide::Left,
        line: 12,
        start_line: None,
        body: "Why?".into(),
    };
    let created = client.add_thread("PR_1", Some("R_1"), &single).unwrap();
    assert_eq!(created.id, "T5");
    assert_eq!(
        variables(&server, 0),
        json!({ "input": {
            "pullRequestId": "PR_1", "pullRequestReviewId": "R_1", "path": "src/A.kt",
            "body": "Why?", "line": 12, "side": "LEFT",
        } })
    );

    let range = NewThread {
        start_line: Some(10),
        side: DiffSide::Right,
        ..single
    };
    client.add_thread("PR_1", None, &range).unwrap();
    assert_eq!(
        variables(&server, 1),
        json!({ "input": {
            "pullRequestId": "PR_1", "path": "src/A.kt", "body": "Why?", "line": 12,
            "side": "RIGHT", "startLine": 10, "startSide": "RIGHT",
        } })
    );
}

#[test]
fn add_thread_with_null_payload_is_a_decode_error() {
    let (_server, client) = client(vec![
        r#"{"data":{"addPullRequestReviewThread":{"thread":null}}}"#.to_string(),
    ]);
    let new = NewThread {
        path: "p".into(),
        side: DiffSide::Right,
        line: 1,
        start_line: None,
        body: "b".into(),
    };
    assert!(matches!(
        client.add_thread("PR_1", None, &new),
        Err(ReviewError::Decode(_))
    ));
}

#[test]
fn reply_sends_the_input_and_decodes_the_comment() {
    let comment = r#"{"id":"C2","body":"Because","createdAt":"d","url":"u","author":{"login":"me"},
        "pullRequestReview":{"state":"PENDING"}}"#;
    let (server, client) = client(vec![
        format!(r#"{{"data":{{"addPullRequestReviewThreadReply":{{"comment":{comment}}}}}}}"#),
        format!(r#"{{"data":{{"addPullRequestReviewThreadReply":{{"comment":{comment}}}}}}}"#),
        r#"{"data":{"addPullRequestReviewThreadReply":{"comment":null}}}"#.to_string(),
    ]);
    let reply = client.reply("T1", Some("R_1"), "Because").unwrap();
    assert_eq!(reply.id, "C2");
    assert_eq!(reply.author, "me");
    assert!(reply.pending);
    assert_eq!(
        variables(&server, 0),
        json!({ "input": {
            "pullRequestReviewThreadId": "T1", "pullRequestReviewId": "R_1", "body": "Because",
        } })
    );
    client.reply("T1", None, "x").unwrap();
    assert_eq!(
        variables(&server, 1),
        json!({ "input": { "pullRequestReviewThreadId": "T1", "body": "x" } })
    );
    assert!(matches!(
        client.reply("T1", None, "x"),
        Err(ReviewError::Decode(_))
    ));
}

#[test]
fn resolve_and_unresolve_send_the_thread_id() {
    let (server, client) = client(vec![
        r#"{"data":{"resolveReviewThread":{"thread":{"id":"T1"}}}}"#.to_string(),
        r#"{"data":{"unresolveReviewThread":{"thread":{"id":"T1"}}}}"#.to_string(),
    ]);
    client.resolve_thread("T1").unwrap();
    client.unresolve_thread("T1").unwrap();
    assert_eq!(
        variables(&server, 0),
        json!({ "input": { "threadId": "T1" } })
    );
    assert_eq!(
        variables(&server, 1),
        json!({ "input": { "threadId": "T1" } })
    );
    let queries: Vec<_> = server
        .requests()
        .iter()
        .map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["query"].to_string())
        .collect();
    assert!(queries[0].contains("resolveReviewThread"));
    assert!(queries[1].contains("unresolveReviewThread"));
}

#[test]
fn api_errors_on_mutations_surface() {
    let (_server, client) = client(vec![
        r#"{"data":null,"errors":[{"message":"Could not resolve to a node"}]}"#.to_string(),
    ]);
    assert!(matches!(
        client.resolve_thread("nope"),
        Err(ReviewError::Api(m)) if m.contains("Could not resolve")
    ));
}
