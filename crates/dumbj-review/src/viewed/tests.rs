use serde_json::{Value, json};

use crate::ReviewError;
use crate::testing::{client, repo, variables};

#[test]
fn collects_viewed_paths_across_pages() {
    let (server, client) = client(vec![
        r#"{"data":{"repository":{"pullRequest":{"files":{
          "pageInfo":{"hasNextPage":true,"endCursor":"c1"},
          "nodes":[{"path":"a","viewerViewedState":"VIEWED"},{"path":"b","viewerViewedState":"UNVIEWED"}]}}}}}"#,
        r#"{"data":{"repository":{"pullRequest":{"files":{
          "pageInfo":{"hasNextPage":false,"endCursor":"c2"},
          "nodes":[{"path":"c","viewerViewedState":"DISMISSED"},{"path":"d","viewerViewedState":"VIEWED"}]}}}}}"#,
    ]);
    assert_eq!(client.viewed_files(&repo(), 4).unwrap(), ["a", "d"]);
    assert_eq!(
        variables(&server, 0),
        json!({ "owner": "o", "name": "r", "number": 4, "after": null })
    );
    assert_eq!(variables(&server, 1)["after"], "c1");
}

#[test]
fn a_page_without_cursor_is_a_decode_error() {
    let (_server, client) = client(vec![
        r#"{"data":{"repository":{"pullRequest":{"files":{
          "pageInfo":{"hasNextPage":true,"endCursor":null},"nodes":[]}}}}}"#,
    ]);
    assert!(matches!(
        client.viewed_files(&repo(), 4),
        Err(ReviewError::Decode(_))
    ));
}

#[test]
fn mark_and_unmark_send_the_path() {
    let (server, client) = client(vec![
        r#"{"data":{"markFileAsViewed":{"clientMutationId":null}}}"#,
        r#"{"data":{"unmarkFileAsViewed":{"clientMutationId":null}}}"#,
        r#"{"data":null,"errors":[{"message":"denied"}]}"#,
    ]);
    client.mark_file_viewed("PR_1", "src/A.kt").unwrap();
    client.unmark_file_viewed("PR_1", "src/A.kt").unwrap();
    let expected = json!({ "input": { "pullRequestId": "PR_1", "path": "src/A.kt" } });
    assert_eq!(variables(&server, 0), expected);
    assert_eq!(variables(&server, 1), expected);
    let first: Value = serde_json::from_str(&server.requests()[0].body).unwrap();
    assert!(
        first["query"]
            .as_str()
            .unwrap()
            .contains("markFileAsViewed")
    );
    let second: Value = serde_json::from_str(&server.requests()[1].body).unwrap();
    assert!(
        second["query"]
            .as_str()
            .unwrap()
            .contains("unmarkFileAsViewed")
    );
    assert!(matches!(
        client.mark_file_viewed("PR_1", "x"),
        Err(ReviewError::Api(m)) if m == "denied"
    ));
}
