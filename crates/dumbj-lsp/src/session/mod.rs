use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification,
    PublishDiagnostics,
};
use lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, VersionedTextDocumentIdentifier,
};
use serde_json::Value;

use crate::backoff::Backoff;
use crate::error::LspError;
use crate::preset::{ServerKind, initialize_params};
use crate::server::{Server, ServerCommand, ServerEvent};
use crate::uri::{path_to_uri, uri_to_path};

const POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Starting,
    Indexing(Option<String>),
    Ready,
    Restarting,
    Failed(String),
}

type EventSink = Box<dyn Fn(ServerEvent) + Send>;
type Connector = Box<dyn Fn(EventSink) -> Result<Server, LspError> + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub initialize_timeout: Duration,
    pub healthy_after: Duration,
    pub delay_divisor: u32,
}

impl Timing {
    const PRODUCTION: Timing = Timing {
        initialize_timeout: Duration::from_secs(120),
        healthy_after: Duration::from_secs(60),
        delay_divisor: 1,
    };
}

#[derive(Default)]
struct Progress {
    tokens: Vec<String>,
    text: Option<String>,
    status: Option<String>,
    service_ready: bool,
}

impl Progress {
    fn apply(&mut self, method: &str, params: &Value) {
        match method {
            "$/progress" => self.apply_progress(params),
            "language/status" => {
                let message = params.get("message").and_then(Value::as_str);
                match params.get("type").and_then(Value::as_str) {
                    Some("Starting") => self.status = message.map(str::to_owned),
                    Some("ServiceReady") => {
                        self.service_ready = true;
                        self.status = None;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn apply_progress(&mut self, params: &Value) {
        let Some(token) = params.get("token").map(Value::to_string) else {
            return;
        };
        let value = params.get("value");
        let field = |name: &str| {
            value
                .and_then(|v| v.get(name))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match value.and_then(|v| v.get("kind")).and_then(Value::as_str) {
            Some("begin") => {
                if !self.tokens.contains(&token) {
                    self.tokens.push(token);
                }
                self.text = field("message").or_else(|| field("title"));
            }
            Some("report") => {
                if let Some(message) = field("message") {
                    self.text = Some(message);
                }
            }
            Some("end") => {
                self.tokens.retain(|t| *t != token);
                if self.tokens.is_empty() {
                    self.text = None;
                }
            }
            _ => {}
        }
    }

    fn state(&self, kind: ServerKind) -> SessionState {
        let waiting = kind == ServerKind::Jdtls && !self.service_ready;
        if self.tokens.is_empty() && !waiting {
            SessionState::Ready
        } else {
            SessionState::Indexing(self.text.clone().or_else(|| self.status.clone()))
        }
    }
}

struct Shared {
    kind: ServerKind,
    root: PathBuf,
    connector: Connector,
    timing: Timing,
    on_change: Box<dyn Fn() + Send + Sync>,
    state: Mutex<SessionState>,
    server: Mutex<Option<Server>>,
    documents: Mutex<HashMap<PathBuf, (i32, String)>>,
    diagnostics: Mutex<HashMap<PathBuf, Arc<Vec<Diagnostic>>>>,
    legend: Mutex<Option<lsp_types::SemanticTokensLegend>>,
    stopped: AtomicBool,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("kind", &self.kind)
            .field("root", &self.root)
            .field("state", &*lock(&self.state))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    fn set_state(&self, new: SessionState) {
        {
            let mut state = lock(&self.state);
            if *state == new {
                return;
            }
            *state = new;
        }
        (self.on_change)();
    }

    fn publish_diagnostics(&self, params: Value) {
        let params = match serde_json::from_value::<PublishDiagnosticsParams>(params) {
            Ok(params) => params,
            Err(error) => {
                tracing::debug!(%error, "unreadable diagnostics");
                return;
            }
        };
        let Some(path) = uri_to_path(&params.uri) else {
            return;
        };
        {
            let mut diagnostics = lock(&self.diagnostics);
            if params.diagnostics.is_empty() {
                if diagnostics.remove(&path).is_none() {
                    return;
                }
            } else {
                diagnostics.insert(path, Arc::new(params.diagnostics));
            }
        }
        (self.on_change)();
    }

    fn clear_diagnostics(&self) {
        let had = !std::mem::take(&mut *lock(&self.diagnostics)).is_empty();
        if had {
            (self.on_change)();
        }
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn open_notification(
        &self,
        path: &Path,
        version: i32,
        text: &str,
    ) -> DidOpenTextDocumentParams {
        DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: path_to_uri(path),
                language_id: self.kind.language_id().to_owned(),
                version,
                text: text.to_owned(),
            },
        }
    }

    fn notify_open(&self, server: &Server, path: &Path, version: i32, text: &str) {
        let params = self.open_notification(path, version, text);
        if let Err(error) = server.notify::<DidOpenTextDocument>(params) {
            tracing::debug!(%error, "didOpen failed");
        }
    }

    fn stop(&self) -> Option<Server> {
        self.stopped.store(true, Ordering::SeqCst);
        lock(&self.server).take()
    }

    fn sleep(&self, delay: Duration) {
        let deadline = Instant::now() + delay;
        while !self.stopped() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            std::thread::sleep(left.min(POLL));
        }
    }

    fn retry(&self, backoff: &mut Backoff, reason: &str) -> bool {
        if self.stopped() {
            return false;
        }
        let Some(delay) = backoff.next_delay() else {
            self.set_state(SessionState::Failed(reason.to_owned()));
            return false;
        };
        self.set_state(SessionState::Restarting);
        self.sleep(delay / self.timing.delay_divisor);
        !self.stopped()
    }

    fn supervise(&self) {
        let label = self.kind.label();
        let mut backoff = Backoff::new();
        while !self.stopped() {
            let (tx, rx) = mpsc::channel();
            let sink: EventSink = Box::new(move |event| {
                let _ = tx.send(event);
            });
            let server = match (self.connector)(sink) {
                Ok(server) => server,
                Err(error) => {
                    tracing::warn!(%error, label, "could not start the language server");
                    if self.retry(&mut backoff, &format!("could not start {label}")) {
                        continue;
                    }
                    return;
                }
            };
            let params = initialize_params(self.kind, &self.root);
            let initialized = match server.initialize(params, self.timing.initialize_timeout) {
                Ok(initialized) => initialized,
                Err(error) => {
                    tracing::warn!(%error, label, "language server did not initialize");
                    drop(server);
                    if self.retry(&mut backoff, &format!("{label} did not start")) {
                        continue;
                    }
                    return;
                }
            };
            *lock(&self.legend) = match initialized.capabilities.semantic_tokens_provider {
                Some(lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                    options,
                )) => Some(options.legend),
                Some(
                    lsp_types::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                        options,
                    ),
                ) => Some(options.semantic_tokens_options.legend),
                None => None,
            };
            let connected = Instant::now();
            let mut progress = Progress::default();
            {
                let documents = lock(&self.documents);
                if self.stopped() {
                    drop(documents);
                    server.shutdown(Duration::from_secs(2));
                    return;
                }
                for (path, (version, text)) in documents.iter() {
                    self.notify_open(&server, path, *version, text);
                }
                *lock(&self.server) = Some(server.clone());
            }
            self.set_state(progress.state(self.kind));
            let stopped = self.run(&rx, &mut progress, &mut backoff);
            lock(&self.server).take();
            self.clear_diagnostics();
            if stopped {
                server.shutdown(Duration::from_secs(2));
                return;
            }
            drop(server);
            if connected.elapsed() >= self.timing.healthy_after {
                backoff.reset();
            }
            if !self.retry(&mut backoff, &format!("{label} keeps crashing")) {
                return;
            }
        }
    }

    fn run(
        &self,
        events: &mpsc::Receiver<ServerEvent>,
        progress: &mut Progress,
        backoff: &mut Backoff,
    ) -> bool {
        loop {
            match events.recv_timeout(POLL) {
                Ok(ServerEvent::Notification { method, params })
                    if method == PublishDiagnostics::METHOD =>
                {
                    self.publish_diagnostics(params);
                }
                Ok(ServerEvent::Notification { method, params }) => {
                    progress.apply(&method, &params);
                    let state = progress.state(self.kind);
                    if state == SessionState::Ready {
                        backoff.reset();
                    }
                    self.set_state(state);
                }
                Ok(ServerEvent::Exited) | Err(RecvTimeoutError::Disconnected) => {
                    return self.stopped();
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.stopped() {
                        return true;
                    }
                }
            }
        }
    }
}

struct StopOnDrop(Arc<Shared>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(server) = self.0.stop() {
            let spawned = std::thread::Builder::new()
                .name("lsp-stop".into())
                .spawn(move || server.shutdown(Duration::from_secs(2)));
            if let Err(error) = spawned {
                tracing::debug!(%error, "could not start the shutdown thread");
            }
        }
    }
}

/// Dropping the last clone stops the server.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
    _guard: Arc<StopOnDrop>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shared.fmt(f)
    }
}

impl Session {
    pub fn start(
        kind: ServerKind,
        command: ServerCommand,
        root: PathBuf,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Session {
        Self::with_connector(
            kind,
            root,
            Box::new(move |sink| Server::spawn(&command, sink)),
            Timing::PRODUCTION,
            Box::new(on_change),
        )
    }

    pub(crate) fn with_connector(
        kind: ServerKind,
        root: PathBuf,
        connector: Connector,
        timing: Timing,
        on_change: Box<dyn Fn() + Send + Sync>,
    ) -> Session {
        let shared = Arc::new(Shared {
            kind,
            root,
            connector,
            timing,
            on_change,
            state: Mutex::new(SessionState::Starting),
            server: Mutex::new(None),
            documents: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(HashMap::new()),
            legend: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("lsp-supervisor".into())
            .spawn(move || thread_shared.supervise());
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start the LSP supervisor thread");
            shared.set_state(SessionState::Failed("could not start a thread".to_owned()));
        }
        Session {
            _guard: Arc::new(StopOnDrop(Arc::clone(&shared))),
            shared,
        }
    }

    #[must_use]
    pub fn kind(&self) -> ServerKind {
        self.shared.kind
    }

    #[must_use]
    pub fn state(&self) -> SessionState {
        lock(&self.shared.state).clone()
    }

    #[must_use]
    pub fn server(&self) -> Option<Server> {
        lock(&self.shared.server).clone()
    }

    #[must_use]
    pub fn diagnostics(&self, path: &Path) -> Arc<Vec<Diagnostic>> {
        lock(&self.shared.diagnostics)
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn semantic_legend(&self) -> Option<lsp_types::SemanticTokensLegend> {
        lock(&self.shared.legend).clone()
    }

    pub fn open_document(&self, path: &Path, text: String) {
        let shared = &self.shared;
        let mut documents = lock(&shared.documents);
        let (version, text) = documents
            .entry(path.to_path_buf())
            .insert_entry((1, text))
            .into_mut()
            .clone();
        if let Some(server) = self.server() {
            shared.notify_open(&server, path, version, &text);
        }
    }

    pub fn change_document(&self, path: &Path, text: String) {
        let shared = &self.shared;
        let mut documents = lock(&shared.documents);
        let Some((version, current)) = documents.get_mut(path) else {
            drop(documents);
            self.open_document(path, text);
            return;
        };
        if *current == text {
            return;
        }
        *version += 1;
        current.clone_from(&text);
        let version = *version;
        if let Some(server) = self.server() {
            let params = DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: path_to_uri(path),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text,
                }],
            };
            if let Err(error) = server.notify::<DidChangeTextDocument>(params) {
                tracing::debug!(%error, "didChange failed");
            }
        }
    }

    pub fn close_document(&self, path: &Path) {
        let mut documents = lock(&self.shared.documents);
        if documents.remove(path).is_none() {
            return;
        }
        if let Some(server) = self.server() {
            let params = DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier {
                    uri: path_to_uri(path),
                },
            };
            if let Err(error) = server.notify::<DidCloseTextDocument>(params) {
                tracing::debug!(%error, "didClose failed");
            }
        }
    }

    pub fn stop(&self) {
        if let Some(server) = self.shared.stop() {
            server.shutdown(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests;
