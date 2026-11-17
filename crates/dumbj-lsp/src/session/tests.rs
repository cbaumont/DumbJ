use std::io::{BufReader, PipeWriter};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::frame::{read_frame, write_frame};

const WAIT: Duration = Duration::from_secs(5);

struct Mock {
    incoming: Receiver<Value>,
    to_client: PipeWriter,
}

impl Mock {
    fn recv(&self) -> Value {
        self.incoming.recv_timeout(WAIT).unwrap()
    }

    fn send(&mut self, value: &Value) {
        write_frame(&mut self.to_client, &serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn notify(&mut self, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn handshake(&mut self) {
        let request = self.recv();
        assert_eq!(request["method"], "initialize");
        self.send(&json!({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {}}}));
        assert_eq!(self.recv()["method"], "initialized");
    }

    fn progress(&mut self, token: u32, kind: &str, title: Option<&str>, message: Option<&str>) {
        let mut value = json!({"kind": kind});
        if let Some(title) = title {
            value["title"] = json!(title);
        }
        if let Some(message) = message {
            value["message"] = json!(message);
        }
        self.notify("$/progress", &json!({"token": token, "value": value}));
    }
}

fn mock_pair(sink: EventSink) -> (Server, Mock) {
    let (client_in, to_client) = std::io::pipe().unwrap();
    let (from_client, client_out) = std::io::pipe().unwrap();
    let server = Server::connect(client_in, client_out, sink);
    let (tx, incoming) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from_client);
        while let Ok(Some(body)) = read_frame(&mut reader) {
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                break;
            }
        }
    });
    (
        server,
        Mock {
            incoming,
            to_client,
        },
    )
}

fn timing() -> Timing {
    Timing {
        initialize_timeout: WAIT,
        healthy_after: Duration::from_secs(60),
        delay_divisor: 1000,
    }
}

struct Harness {
    session: Session,
    mocks: Receiver<Mock>,
    changes: Receiver<()>,
}

fn harness(kind: ServerKind, timing: Timing) -> Harness {
    let (mock_tx, mocks) = mpsc::channel();
    let (change_tx, changes) = mpsc::channel();
    let mock_tx = Mutex::new(mock_tx);
    let change_tx = Mutex::new(change_tx);
    let session = Session::with_connector(
        kind,
        "/work/proj".into(),
        Box::new(move |sink| {
            let (server, mock) = mock_pair(sink);
            let _ = mock_tx.lock().unwrap().send(mock);
            Ok(server)
        }),
        timing,
        Box::new(move || {
            let _ = change_tx.lock().unwrap().send(());
        }),
    );
    Harness {
        session,
        mocks,
        changes,
    }
}

impl Harness {
    fn mock(&self) -> Mock {
        self.mocks.recv_timeout(WAIT).unwrap()
    }

    fn ready_mock(&self) -> Mock {
        let mut mock = self.mock();
        mock.handshake();
        mock
    }

    fn wait_for(&self, expected: &SessionState) {
        let deadline = Instant::now() + WAIT;
        while self.session.state() != *expected {
            assert!(
                Instant::now() < deadline,
                "wanted {expected:?}, state is {:?}",
                self.session.state()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn indexing(text: &str) -> SessionState {
    SessionState::Indexing(Some(text.to_owned()))
}

#[test]
fn kotlin_goes_from_starting_to_ready() {
    let h = harness(ServerKind::KotlinLsp, timing());
    assert_eq!(h.session.state(), SessionState::Starting);
    assert!(h.session.server().is_none());
    let mut mock = h.mock();
    assert_eq!(h.session.state(), SessionState::Starting);
    let request = mock.recv();
    assert_eq!(request["method"], "initialize");
    assert_eq!(request["params"]["clientInfo"]["name"], "DumbJ");
    mock.send(&json!({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {}}}));
    h.wait_for(&SessionState::Ready);
    assert!(h.session.server().is_some());
    assert_eq!(h.session.kind(), ServerKind::KotlinLsp);
}

#[test]
fn jdtls_is_ready_only_after_service_ready() {
    let h = harness(ServerKind::Jdtls, timing());
    let mut mock = h.ready_mock();
    h.wait_for(&SessionState::Indexing(None));
    assert!(h.session.server().is_some());
    mock.notify(
        "language/status",
        &json!({"type": "Starting", "message": "Importing projects"}),
    );
    h.wait_for(&indexing("Importing projects"));
    mock.progress(1, "begin", Some("Building"), None);
    mock.notify(
        "language/status",
        &json!({"type": "ServiceReady", "message": "Ready"}),
    );
    h.wait_for(&indexing("Building"));
    mock.progress(1, "end", None, None);
    h.wait_for(&SessionState::Ready);
}

#[test]
fn progress_drives_indexing_text() {
    let h = harness(ServerKind::KotlinLsp, timing());
    let mut mock = h.ready_mock();
    h.wait_for(&SessionState::Ready);
    mock.progress(7, "begin", Some("Indexing"), None);
    h.wait_for(&indexing("Indexing"));
    mock.progress(7, "report", None, Some("40%"));
    h.wait_for(&indexing("40%"));
    mock.progress(8, "begin", Some("Resolving"), Some("deps"));
    h.wait_for(&indexing("deps"));
    mock.progress(7, "end", None, None);
    mock.progress(8, "end", None, None);
    h.wait_for(&SessionState::Ready);
    mock.progress(9, "begin", Some("Again"), None);
    h.wait_for(&indexing("Again"));
    mock.progress(9, "end", None, None);
    h.wait_for(&SessionState::Ready);
}

#[test]
fn documents_are_synchronised() {
    let h = harness(ServerKind::KotlinLsp, timing());
    let a = Path::new("/work/proj/A.kt");
    let b = Path::new("/work/proj/B.kt");
    h.session.open_document(a, "one".into());
    h.session.change_document(a, "two".into());
    h.session.open_document(b, "bee".into());
    let mut mock = h.mock();
    mock.handshake();
    let mut opened = [mock.recv(), mock.recv()];
    opened.sort_by_key(|m| m["params"]["textDocument"]["uri"].to_string());
    assert_eq!(opened[0]["method"], "textDocument/didOpen");
    let item = &opened[0]["params"]["textDocument"];
    assert_eq!(item["uri"], "file:///work/proj/A.kt");
    assert_eq!(item["languageId"], "kotlin");
    assert_eq!(item["version"], 2);
    assert_eq!(item["text"], "two");
    assert_eq!(opened[1]["params"]["textDocument"]["text"], "bee");
    h.wait_for(&SessionState::Ready);

    h.session.change_document(a, "two".into());
    h.session.change_document(a, "three".into());
    let change = mock.recv();
    assert_eq!(change["method"], "textDocument/didChange");
    assert_eq!(change["params"]["textDocument"]["version"], 3);
    assert_eq!(
        change["params"]["contentChanges"],
        json!([{"text": "three"}])
    );

    let c = Path::new("/work/proj/C.kt");
    h.session.change_document(c, "see".into());
    let opened = mock.recv();
    assert_eq!(opened["method"], "textDocument/didOpen");
    assert_eq!(opened["params"]["textDocument"]["version"], 1);

    h.session.close_document(a);
    h.session.close_document(a);
    let closed = mock.recv();
    assert_eq!(closed["method"], "textDocument/didClose");
    assert_eq!(
        closed["params"]["textDocument"]["uri"],
        "file:///work/proj/A.kt"
    );
}

#[test]
fn java_documents_use_the_java_language_id() {
    let h = harness(ServerKind::Jdtls, timing());
    let mock = h.ready_mock();
    h.session
        .open_document(Path::new("/work/proj/A.java"), "class A {}".into());
    let opened = mock.recv();
    assert_eq!(opened["params"]["textDocument"]["languageId"], "java");
}

#[test]
fn a_crashed_server_is_restarted_with_its_documents() {
    let mut timing = timing();
    timing.delay_divisor = 20;
    let h = harness(ServerKind::KotlinLsp, timing);
    let a = Path::new("/work/proj/A.kt");
    h.session.open_document(a, "one".into());
    let mut first = h.mock();
    first.handshake();
    assert_eq!(first.recv()["method"], "textDocument/didOpen");
    h.wait_for(&SessionState::Ready);
    h.session.change_document(a, "two".into());
    assert_eq!(first.recv()["method"], "textDocument/didChange");
    drop(first);
    h.wait_for(&SessionState::Restarting);
    assert!(h.session.server().is_none());
    let mut second = h.mock();
    second.handshake();
    let reopened = second.recv();
    assert_eq!(reopened["method"], "textDocument/didOpen");
    assert_eq!(reopened["params"]["textDocument"]["text"], "two");
    h.wait_for(&SessionState::Ready);
}

#[test]
fn a_connector_that_always_fails_ends_failed() {
    let attempts = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&attempts);
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let session = Session::with_connector(
        ServerKind::Jdtls,
        "/work/proj".into(),
        Box::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(LspError::Io(std::io::Error::other("no such binary")))
        }),
        timing(),
        Box::new(move || {
            let _ = tx.lock().unwrap().send(());
        }),
    );
    let deadline = Instant::now() + WAIT;
    while !matches!(session.state(), SessionState::Failed(_)) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        session.state(),
        SessionState::Failed("could not start jdtls".into())
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 6);
    assert!(session.server().is_none());
    assert!(rx.try_iter().count() >= 2);
}

#[test]
fn a_hung_initialize_is_abandoned() {
    let mut timing = timing();
    timing.initialize_timeout = Duration::from_millis(60);
    let h = harness(ServerKind::KotlinLsp, timing);
    let hung = h.mock();
    assert_eq!(hung.recv()["method"], "initialize");
    let mut second = h.mock();
    second.handshake();
    h.wait_for(&SessionState::Ready);
    drop(hung);
}

#[test]
fn stop_shuts_down_and_does_not_reconnect() {
    let h = harness(ServerKind::KotlinLsp, timing());
    let mut mock = h.ready_mock();
    h.wait_for(&SessionState::Ready);
    let session = h.session.clone();
    let stopper = std::thread::spawn(move || session.stop());
    let shutdown = mock.recv();
    assert_eq!(shutdown["method"], "shutdown");
    mock.send(&json!({"jsonrpc": "2.0", "id": shutdown["id"], "result": null}));
    assert_eq!(mock.recv()["method"], "exit");
    stopper.join().unwrap();
    drop(mock);
    std::thread::sleep(Duration::from_millis(80));
    assert!(h.mocks.try_recv().is_err());
    assert!(h.session.server().is_none());
    assert_eq!(h.session.state(), SessionState::Ready);
}

#[test]
fn dropping_the_last_session_shuts_the_server_down() {
    let h = harness(ServerKind::KotlinLsp, timing());
    let mut mock = h.ready_mock();
    h.wait_for(&SessionState::Ready);
    let Harness { session, mocks, .. } = h;
    let clone = session.clone();
    drop(session);
    std::thread::sleep(Duration::from_millis(30));
    assert!(mock.incoming.try_recv().is_err());
    drop(clone);
    let shutdown = mock.recv();
    assert_eq!(shutdown["method"], "shutdown");
    mock.send(&json!({"jsonrpc": "2.0", "id": shutdown["id"], "result": null}));
    assert_eq!(mock.recv()["method"], "exit");
    drop(mock);
    std::thread::sleep(Duration::from_millis(80));
    assert!(mocks.try_recv().is_err());
}

#[test]
fn on_change_fires_only_on_transitions() {
    let h = harness(ServerKind::KotlinLsp, timing());
    let mut mock = h.ready_mock();
    h.wait_for(&SessionState::Ready);
    assert_eq!(h.changes.try_iter().count(), 1);
    mock.notify("window/logMessage", &json!({"type": 3, "message": "hi"}));
    mock.progress(1, "begin", Some("Work"), None);
    h.wait_for(&indexing("Work"));
    assert_eq!(h.changes.try_iter().count(), 1);
    mock.progress(1, "report", None, Some("Work"));
    mock.progress(1, "end", None, None);
    h.wait_for(&SessionState::Ready);
    assert_eq!(h.changes.try_iter().count(), 1);
}

#[test]
fn diagnostics_are_kept_per_file_and_dropped_with_the_server() {
    let harness = harness(ServerKind::KotlinLsp, timing());
    let mut mock = harness.ready_mock();
    let path = Path::new("/work/proj/A.kt");
    let diagnostic = json!({"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}, "severity": 1, "message": "Unresolved reference"});
    mock.notify(
        "textDocument/publishDiagnostics",
        &json!({"uri": "file:///work/proj/A.kt", "diagnostics": [diagnostic]}),
    );
    let deadline = Instant::now() + WAIT;
    while harness.session.diagnostics(path).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        harness.session.diagnostics(path)[0].message,
        "Unresolved reference"
    );
    let version = harness.session.diagnostics_version();
    drop(mock);
    let deadline = Instant::now() + WAIT;
    while !harness.session.diagnostics(path).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(harness.session.diagnostics(path).is_empty());
    assert!(harness.session.diagnostics_version() > version);
}
