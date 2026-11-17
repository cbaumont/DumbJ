use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use lsp_types::request::{GotoDefinition, Request};
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Position, TextDocumentIdentifier};
use lsp_types::{TextDocumentPositionParams, Uri};
use serde_json::{Value, json};

use crate::testing::{Mock, WAIT, mock_pair};
use crate::{LspError, Server, ServerCommand, ServerEvent};

fn connect() -> (Server, Mock, Receiver<ServerEvent>) {
    let (events_tx, events) = mpsc::channel();
    let (server, mock) = mock_pair(move |event| {
        let _ = events_tx.send(event);
    });
    (server, mock, events)
}

fn definition_params() -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///a.kt".parse::<Uri>().unwrap(),
            },
            position: Position::new(1, 2),
        },
        work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        partial_result_params: lsp_types::PartialResultParams::default(),
    }
}

fn location(line: u32) -> Value {
    json!({"uri": "file:///b.kt", "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 1}}})
}

#[test]
fn request_round_trip() {
    let (server, mut mock, _events) = connect();
    let pending = server.request::<GotoDefinition>(definition_params());
    let request = mock.recv();
    assert_eq!(request["method"], GotoDefinition::METHOD);
    assert_eq!(request["params"]["position"]["line"], 1);
    mock.reply(&request["id"], &location(7));
    let Some(GotoDefinitionResponse::Scalar(found)) = pending.wait(WAIT).unwrap() else {
        panic!("expected a scalar location");
    };
    assert_eq!(found.range.start.line, 7);
}

#[test]
fn error_response() {
    let (server, mut mock, _events) = connect();
    let pending = server.request::<GotoDefinition>(definition_params());
    let id = mock.recv()["id"].clone();
    mock.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "boom"}}));
    match pending.wait(WAIT) {
        Err(LspError::Response { code, message }) => {
            assert_eq!((code, message.as_str()), (-32603, "boom"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn bad_result_is_a_decode_error() {
    let (server, mut mock, _events) = connect();
    let pending = server.request::<GotoDefinition>(definition_params());
    let id = mock.recv()["id"].clone();
    mock.reply(&id, &json!(42));
    assert!(matches!(pending.wait(WAIT), Err(LspError::Decode(_))));
}

#[test]
fn timeout_cancels_and_late_answer_is_ignored() {
    let (server, mut mock, _events) = connect();
    let pending = server.request::<GotoDefinition>(definition_params());
    let id = mock.recv()["id"].clone();
    assert!(matches!(
        pending.wait(Duration::from_millis(50)),
        Err(LspError::Timeout)
    ));
    let cancel = mock.recv();
    assert_eq!(cancel["method"], "$/cancelRequest");
    assert_eq!(cancel["params"]["id"], id);
    mock.reply(&id, &location(1));
    let next = server.request::<GotoDefinition>(definition_params());
    let next_id = mock.recv()["id"].clone();
    mock.reply(&next_id, &location(2));
    assert!(next.wait(WAIT).unwrap().is_some());
    assert!(server.is_alive());
}

#[test]
fn concurrent_requests_answered_out_of_order() {
    let (server, mut mock, _events) = connect();
    let first = server.request::<GotoDefinition>(definition_params());
    let second = server.request::<GotoDefinition>(definition_params());
    let id_first = mock.recv()["id"].clone();
    let id_second = mock.recv()["id"].clone();
    mock.reply(&id_second, &location(2));
    mock.reply(&id_first, &location(1));
    let line = |response: Option<GotoDefinitionResponse>| match response {
        Some(GotoDefinitionResponse::Scalar(l)) => l.range.start.line,
        other => panic!("{other:?}"),
    };
    assert_eq!(line(first.wait(WAIT).unwrap()), 1);
    assert_eq!(line(second.wait(WAIT).unwrap()), 2);
}

#[test]
fn notifications_reach_the_callback() {
    let (_server, mut mock, events) = connect();
    mock.send(&json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": 1}}));
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        ServerEvent::Notification {
            method: "$/progress".into(),
            params: json!({"token": 1}),
        }
    );
}

#[test]
fn server_requests_are_answered() {
    let (_server, mut mock, _events) = connect();
    for method in [
        "window/workDoneProgress/create",
        "client/registerCapability",
        "client/unregisterCapability",
    ] {
        mock.send(&json!({"jsonrpc": "2.0", "id": 10, "method": method, "params": {}}));
        let answer = mock.recv();
        assert_eq!(answer["id"], 10);
        assert!(answer["result"].is_null() && answer.get("error").is_none());
    }
    mock.send(
        &json!({"jsonrpc": "2.0", "id": "c", "method": "workspace/configuration",
        "params": {"items": [{"section": "a"}, {"section": "b"}, {}]}}),
    );
    let answer = mock.recv();
    assert_eq!(answer["id"], "c");
    assert_eq!(answer["result"], json!([null, null, null]));
    mock.send(&json!({"jsonrpc": "2.0", "id": 11, "method": "custom/thing"}));
    let answer = mock.recv();
    assert_eq!(answer["id"], 11);
    assert_eq!(answer["error"]["code"], -32601);
}

#[test]
fn closed_stream_fails_pending_and_exits_once() {
    let (server, mock, events) = connect();
    let pending = server.request::<GotoDefinition>(definition_params());
    mock.recv();
    drop(mock);
    assert!(matches!(pending.wait(WAIT), Err(LspError::ServerExited)));
    assert_eq!(events.recv_timeout(WAIT).unwrap(), ServerEvent::Exited);
    assert!(!server.is_alive());
    assert!(matches!(
        server
            .request::<GotoDefinition>(definition_params())
            .wait(WAIT),
        Err(LspError::ServerExited)
    ));
    assert!(matches!(
        server.notify::<lsp_types::notification::Exit>(()),
        Err(LspError::ServerExited)
    ));
    drop(server);
    assert!(events.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn initialize_sends_initialized() {
    let (server, mut mock, _events) = connect();
    let client =
        std::thread::spawn(move || server.initialize(lsp_types::InitializeParams::default(), WAIT));
    let request = mock.recv();
    assert_eq!(request["method"], "initialize");
    mock.reply(
        &request["id"],
        &json!({"capabilities": {"definitionProvider": true}}),
    );
    let notification = mock.recv();
    assert_eq!(notification["method"], "initialized");
    let result = client.join().unwrap().unwrap();
    assert!(result.capabilities.definition_provider.is_some());
}

#[test]
fn shutdown_sends_shutdown_then_exit() {
    let (server, mut mock, _events) = connect();
    let client = std::thread::spawn(move || server.shutdown(WAIT));
    let request = mock.recv();
    assert_eq!(request["method"], "shutdown");
    assert!(request.get("params").is_none());
    mock.reply(&request["id"], &Value::Null);
    assert_eq!(mock.recv()["method"], "exit");
    client.join().unwrap();
}

fn sh(script: &str) -> ServerCommand {
    ServerCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
        cwd: std::env::temp_dir(),
    }
}

#[test]
fn spawned_child_that_exits_reports_exited() {
    let (tx, rx) = mpsc::channel();
    let server = Server::spawn(&sh("exit 3"), move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), ServerEvent::Exited);
    assert!(!server.is_alive());
}

#[test]
fn missing_program_is_an_error() {
    let mut command = sh("");
    command.program = "/nonexistent/dumbj-no-such-server".into();
    assert!(matches!(
        Server::spawn(&command, |_| {}),
        Err(LspError::Io(_))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn dropping_the_server_kills_the_child() {
    let server = Server::spawn(&sh("sleep 60"), |_| {}).unwrap();
    let pid = server.pid().unwrap();
    let proc = std::path::PathBuf::from(format!("/proc/{pid}"));
    assert!(proc.exists());
    let clone = server.clone();
    drop(server);
    assert!(proc.exists());
    drop(clone);
    let deadline = std::time::Instant::now() + WAIT;
    while proc.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!proc.exists());
}

#[test]
fn goto_reads_every_response_shape() {
    use crate::{GotoKind, goto};
    let (server, mut mock, _events) = connect();
    let link = json!([{"targetUri": "file:///c.kt", "targetRange": location(1)["range"], "targetSelectionRange": location(9)["range"]}]);
    let cases = [
        (
            GotoKind::Definition,
            "textDocument/definition",
            location(3),
            vec![3],
        ),
        (
            GotoKind::TypeDefinition,
            "textDocument/typeDefinition",
            json!([location(4), location(5)]),
            vec![4, 5],
        ),
        (
            GotoKind::Implementation,
            "textDocument/implementation",
            link,
            vec![9],
        ),
        (
            GotoKind::Definition,
            "textDocument/definition",
            Value::Null,
            vec![],
        ),
    ];
    for (kind, method, result, lines) in cases {
        let asked = server.clone();
        let call = std::thread::spawn(move || {
            goto(
                &asked,
                kind,
                std::path::Path::new("/a b.kt"),
                Position::new(2, 4),
                WAIT,
            )
        });
        let request = mock.recv();
        assert_eq!(request["method"], method);
        assert_eq!(request["params"]["textDocument"]["uri"], "file:///a%20b.kt");
        assert_eq!(request["params"]["position"]["character"], 4);
        mock.reply(&request["id"], &result);
        let places = call.join().unwrap().unwrap();
        let found: Vec<u32> = places.iter().map(|place| place.range.start.line).collect();
        assert_eq!(found, lines);
    }
}

#[test]
fn references_exclude_the_declaration() {
    let (server, mut mock, _events) = connect();
    let asked = server.clone();
    let call = std::thread::spawn(move || {
        crate::references(
            &asked,
            std::path::Path::new("/a.kt"),
            Position::new(0, 1),
            WAIT,
        )
    });
    let request = mock.recv();
    assert_eq!(request["method"], "textDocument/references");
    assert_eq!(request["params"]["context"]["includeDeclaration"], false);
    mock.reply(&request["id"], &json!([location(2), location(6)]));
    assert_eq!(call.join().unwrap().unwrap().len(), 2);
}

#[test]
fn hover_is_flattened_to_markdown() {
    let (server, mut mock, _events) = connect();
    let cases = [
        (
            json!({"contents": {"kind": "markdown", "value": "**doc**"}}),
            Some("**doc**"),
        ),
        (
            json!({"contents": [{"language": "java", "value": "void f()"}, "text"]}),
            Some("```java\nvoid f()\n```\n\ntext"),
        ),
        (json!({"contents": "  "}), None),
        (Value::Null, None),
    ];
    for (result, expected) in cases {
        let asked = server.clone();
        let call = std::thread::spawn(move || {
            crate::hover(
                &asked,
                std::path::Path::new("/a.kt"),
                Position::new(0, 1),
                WAIT,
            )
        });
        let request = mock.recv();
        assert_eq!(request["method"], "textDocument/hover");
        mock.reply(&request["id"], &result);
        assert_eq!(call.join().unwrap().unwrap().as_deref(), expected);
    }
}

fn hierarchy_item(name: &str) -> Value {
    let range = location(3)["range"].clone();
    json!({"name": name, "kind": 5, "detail": "demo", "uri": "file:///b.kt", "range": range, "selectionRange": range, "data": {"k": 1}})
}

#[test]
fn type_hierarchy_requests() {
    use crate::{subtypes, supertypes, type_hierarchy_prepare};
    let (server, mut mock, _events) = connect();
    let asked = server.clone();
    let call = std::thread::spawn(move || {
        type_hierarchy_prepare(
            &asked,
            std::path::Path::new("/a b.kt"),
            Position::new(2, 4),
            WAIT,
        )
    });
    let request = mock.recv();
    assert_eq!(request["method"], "textDocument/prepareTypeHierarchy");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///a%20b.kt");
    assert_eq!(request["params"]["position"]["character"], 4);
    mock.reply(&request["id"], &json!([hierarchy_item("A")]));
    let items = call.join().unwrap().unwrap();
    assert_eq!(items[0].name, "A");

    for (method, null) in [
        ("typeHierarchy/supertypes", false),
        ("typeHierarchy/subtypes", true),
    ] {
        let asked = server.clone();
        let item = items[0].clone();
        let call = std::thread::spawn(move || {
            if method.ends_with("supertypes") {
                supertypes(&asked, item, WAIT)
            } else {
                subtypes(&asked, item, WAIT)
            }
        });
        let request = mock.recv();
        assert_eq!(request["method"], method);
        assert_eq!(request["params"]["item"]["name"], "A");
        assert_eq!(request["params"]["item"]["data"]["k"], 1);
        let result = if null {
            Value::Null
        } else {
            json!([hierarchy_item("B"), hierarchy_item("C")])
        };
        mock.reply(&request["id"], &result);
        let found = call.join().unwrap().unwrap();
        assert_eq!(found.len(), if null { 0 } else { 2 });
    }
}

#[test]
fn call_hierarchy_requests() {
    use crate::{call_hierarchy_prepare, incoming_calls, outgoing_calls};
    let (server, mut mock, _events) = connect();
    let asked = server.clone();
    let call = std::thread::spawn(move || {
        call_hierarchy_prepare(
            &asked,
            std::path::Path::new("/a.kt"),
            Position::new(2, 4),
            WAIT,
        )
    });
    let request = mock.recv();
    assert_eq!(request["method"], "textDocument/prepareCallHierarchy");
    assert_eq!(request["params"]["position"]["line"], 2);
    mock.reply(&request["id"], &Value::Null);
    assert_eq!(call.join().unwrap().unwrap().len(), 0);

    let item: lsp_types::CallHierarchyItem = serde_json::from_value(hierarchy_item("f")).unwrap();
    let ranges = json!([location(1)["range"]]);
    for (method, key) in [
        ("callHierarchy/incomingCalls", "from"),
        ("callHierarchy/outgoingCalls", "to"),
    ] {
        let asked = server.clone();
        let item = item.clone();
        let call = std::thread::spawn(move || {
            if method.contains("incoming") {
                incoming_calls(&asked, item, WAIT)
            } else {
                outgoing_calls(&asked, item, WAIT)
            }
        });
        let request = mock.recv();
        assert_eq!(request["method"], method);
        assert_eq!(request["params"]["item"]["name"], "f");
        mock.reply(
            &request["id"],
            &json!([{key: hierarchy_item("g"), "fromRanges": ranges}]),
        );
        let found = call.join().unwrap().unwrap();
        assert_eq!(found[0].name, "g");
    }
}

#[test]
fn jdtls_type_items_with_empty_tags_decode() {
    let items: Vec<crate::TypeItem> = serde_json::from_value(json!([{
        "name": "Greeter", "detail": "demo", "kind": 5, "tags": [],
        "uri": "file:///p/Greeter.java",
        "range": {"start": {"line": 2, "character": 0}, "end": {"line": 17, "character": 1}},
        "selectionRange": {"start": {"line": 2, "character": 13}, "end": {"line": 2, "character": 20}},
        "data": {"element": "=demo/src\\/main"}
    }]))
    .unwrap();
    assert_eq!(items[0].name, "Greeter");
    let sent = serde_json::to_value(&items[0]).unwrap();
    assert_eq!(
        sent["data"]["element"], "=demo/src\\/main",
        "data goes back unchanged"
    );
}
