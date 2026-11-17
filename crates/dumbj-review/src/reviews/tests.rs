use serde_json::json;

use crate::testing::{client, variables};
use crate::{ReviewError, ReviewEvent};

#[test]
fn start_review_sends_no_event_and_returns_the_id() {
    let (server, client) = client(vec![
        r#"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R_1"}}}}"#,
        r#"{"data":{"addPullRequestReview":{"pullRequestReview":null}}}"#,
    ]);
    assert_eq!(client.start_review("PR_1").unwrap(), "R_1");
    assert_eq!(
        variables(&server, 0),
        json!({ "input": { "pullRequestId": "PR_1" } })
    );
    assert!(matches!(
        client.start_review("PR_1"),
        Err(ReviewError::Decode(_))
    ));
}

#[test]
fn events_map_to_graphql_values() {
    let ok = r#"{"data":{"submitPullRequestReview":{"pullRequestReview":{"id":"R_1"}}}}"#;
    let (server, client) = client(vec![ok, ok, ok]);
    client
        .submit_review("R_1", ReviewEvent::Comment, "note")
        .unwrap();
    client
        .submit_review("R_1", ReviewEvent::Approve, "")
        .unwrap();
    client
        .submit_review("R_1", ReviewEvent::RequestChanges, "fix")
        .unwrap();
    for (index, event, body) in [
        (0, "COMMENT", "note"),
        (1, "APPROVE", ""),
        (2, "REQUEST_CHANGES", "fix"),
    ] {
        assert_eq!(
            variables(&server, index),
            json!({ "input": {
                "pullRequestReviewId": "R_1", "event": event, "body": body,
            } })
        );
    }
}

#[test]
fn add_review_sends_the_event() {
    let (server, client) = client(vec![
        r#"{"data":{"addPullRequestReview":{"pullRequestReview":{"id":"R_2"}}}}"#,
    ]);
    client
        .add_review("PR_1", ReviewEvent::Approve, "lgtm")
        .unwrap();
    assert_eq!(
        variables(&server, 0),
        json!({ "input": { "pullRequestId": "PR_1", "event": "APPROVE", "body": "lgtm" } })
    );
}

#[test]
fn delete_review_sends_the_id() {
    let (server, client) = client(vec![
        r#"{"data":{"deletePullRequestReview":{"pullRequestReview":{"id":"R_1"}}}}"#,
    ]);
    client.delete_review("R_1").unwrap();
    assert_eq!(
        variables(&server, 0),
        json!({ "input": { "pullRequestReviewId": "R_1" } })
    );
}

#[test]
fn api_errors_surface() {
    let (_server, client) = client(vec![
        r#"{"data":null,"errors":[{"message":"Can not submit a review with no comments"}]}"#,
    ]);
    assert!(matches!(
        client.submit_review("R_1", ReviewEvent::Comment, ""),
        Err(ReviewError::Api(m)) if m.contains("no comments")
    ));
}
