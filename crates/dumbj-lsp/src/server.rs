use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lsp_types::notification::{Exit, Initialized, Notification};
use lsp_types::request::{Initialize, Request, Shutdown};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::LspError;
use crate::frame::{read_frame, write_frame};

const METHOD_NOT_FOUND: i64 = -32601;
const CHILD_POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    Notification { method: String, params: Value },
    Exited,
}

type Answer = Result<Value, LspError>;
type Callback = Box<dyn Fn(ServerEvent) + Send>;

struct Inner {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<i64, mpsc::Sender<Answer>>>,
    next_id: AtomicI64,
    alive: AtomicBool,
    child: Mutex<Option<Child>>,
    on_event: Mutex<Callback>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("alive", &self.alive)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Inner {
    fn send(&self, message: &Value) -> Result<(), LspError> {
        let body = serde_json::to_vec(message).map_err(|e| LspError::Decode(e.to_string()))?;
        let mut writer = lock(&self.writer);
        let writer = writer.as_mut().ok_or(LspError::ServerExited)?;
        write_frame(writer, &body)?;
        Ok(())
    }

    fn finish(&self) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }
        let pending = std::mem::take(&mut *lock(&self.pending));
        for (_, sender) in pending {
            let _ = sender.send(Err(LspError::ServerExited));
        }
        (lock(&self.on_event))(ServerEvent::Exited);
    }

    fn dispatch(&self, body: &[u8]) {
        let Ok(message) = serde_json::from_slice::<Value>(body) else {
            tracing::debug!("dropping a message that is not JSON");
            return;
        };
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id")) {
            (Some(method), Some(id)) => self.answer_request(method, id, &message),
            (Some(method), None) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                (lock(&self.on_event))(ServerEvent::Notification {
                    method: method.to_owned(),
                    params,
                });
            }
            (None, Some(id)) => {
                let Some(sender) = id.as_i64().and_then(|id| lock(&self.pending).remove(&id))
                else {
                    return;
                };
                let _ = sender.send(response_result(&message));
            }
            (None, None) => {}
        }
    }

    fn answer_request(&self, method: &str, id: &Value, message: &Value) {
        let reply = match method {
            "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability" => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            "workspace/configuration" => {
                let items = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                json!({"jsonrpc": "2.0", "id": id, "result": vec![Value::Null; items]})
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": METHOD_NOT_FOUND, "message": format!("method not found: {method}")},
            }),
        };
        if let Err(error) = self.send(&reply) {
            tracing::debug!(%error, method, "could not answer a server request");
        }
    }
}

fn response_result(message: &Value) -> Answer {
    if let Some(error) = message.get("error") {
        return Err(LspError::Response {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    Ok(message.get("result").cloned().unwrap_or(Value::Null))
}

#[derive(Debug)]
struct Handle {
    inner: Arc<Inner>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        let child = lock(&self.inner.child).take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        lock(&self.inner.writer).take();
        self.inner.finish();
    }
}

/// A connection to one language server. Clones share it; a spawned child is killed when the
/// last clone is dropped.
#[derive(Debug, Clone)]
pub struct Server {
    handle: Arc<Handle>,
}

impl Server {
    pub fn connect(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        on_event: impl Fn(ServerEvent) + Send + 'static,
    ) -> Self {
        Self::start(reader, Box::new(writer), Box::new(on_event), None)
    }

    pub fn spawn(
        command: &ServerCommand,
        on_event: impl Fn(ServerEvent) + Send + 'static,
    ) -> Result<Self, LspError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&command.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            return Err(LspError::Io(std::io::Error::other("child pipes missing")));
        };
        std::thread::Builder::new()
            .name("lsp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    tracing::debug!(target: "dumbj_lsp::stderr", "{line}");
                }
            })?;
        let server = Self::start(stdout, Box::new(stdin), Box::new(on_event), Some(child));
        let inner = Arc::clone(&server.handle.inner);
        std::thread::Builder::new()
            .name("lsp-watch".into())
            .spawn(move || watch_child(&inner))?;
        Ok(server)
    }

    fn start(
        reader: impl Read + Send + 'static,
        writer: Box<dyn Write + Send>,
        on_event: Callback,
        child: Option<Child>,
    ) -> Self {
        let inner = Arc::new(Inner {
            writer: Mutex::new(Some(writer)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            alive: AtomicBool::new(true),
            child: Mutex::new(child),
            on_event: Mutex::new(on_event),
        });
        let thread_inner = Arc::clone(&inner);
        let spawned = std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || read_loop(&thread_inner, reader));
        if let Err(error) = spawned {
            tracing::error!(%error, "could not start the LSP reader thread");
            inner.finish();
        }
        Self {
            handle: Arc::new(Handle { inner }),
        }
    }

    fn inner(&self) -> &Arc<Inner> {
        &self.handle.inner
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.inner().alive.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        lock(&self.inner().child).as_ref().map(Child::id)
    }

    pub fn request<R: Request>(&self, params: R::Params) -> Pending<R::Result>
    where
        R::Result: DeserializeOwned,
    {
        let inner = self.inner();
        let (sender, receiver) = mpsc::channel();
        let id = inner.next_id.fetch_add(1, Ordering::SeqCst);
        let mut pending = Pending {
            inner: Arc::clone(inner),
            id,
            receiver,
            early: None,
            done: false,
            _result: PhantomData,
        };
        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(error) => {
                pending.early = Some(LspError::Decode(error.to_string()));
                pending.done = true;
                return pending;
            }
        };
        lock(&inner.pending).insert(id, sender);
        if !inner.alive.load(Ordering::SeqCst) {
            lock(&inner.pending).remove(&id);
            pending.early = Some(LspError::ServerExited);
            pending.done = true;
            return pending;
        }
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": R::METHOD});
        if !params.is_null() {
            message["params"] = params;
        }
        if let Err(error) = inner.send(&message) {
            lock(&inner.pending).remove(&id);
            pending.early = Some(error);
            pending.done = true;
        }
        pending
    }

    pub fn notify<N: Notification>(&self, params: N::Params) -> Result<(), LspError> {
        let params =
            serde_json::to_value(params).map_err(|error| LspError::Decode(error.to_string()))?;
        let inner = self.inner();
        if !inner.alive.load(Ordering::SeqCst) {
            return Err(LspError::ServerExited);
        }
        let mut message = json!({"jsonrpc": "2.0", "method": N::METHOD});
        if !params.is_null() {
            message["params"] = params;
        }
        inner.send(&message)
    }

    pub fn initialize(
        &self,
        params: lsp_types::InitializeParams,
        timeout: Duration,
    ) -> Result<lsp_types::InitializeResult, LspError> {
        let result = self.request::<Initialize>(params).wait(timeout)?;
        self.notify::<Initialized>(lsp_types::InitializedParams {})?;
        Ok(result)
    }

    pub fn shutdown(&self, timeout: Duration) {
        let started = Instant::now();
        if self.is_alive() {
            let _ = self.request::<Shutdown>(()).wait(timeout);
            let _ = self.notify::<Exit>(());
        }
        let inner = self.inner();
        if lock(&inner.child).is_none() {
            return;
        }
        loop {
            let exited = {
                let mut child = lock(&inner.child);
                match child.as_mut() {
                    None => return,
                    Some(child) => child.try_wait().ok().flatten().is_some(),
                }
            };
            if exited {
                return;
            }
            if started.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(CHILD_POLL);
        }
        if let Some(child) = lock(&inner.child).as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn read_loop(inner: &Inner, reader: impl Read) {
    let mut reader = BufReader::new(reader);
    loop {
        match read_frame(&mut reader) {
            Ok(Some(body)) => inner.dispatch(&body),
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(%error, "language server stream failed");
                break;
            }
        }
    }
    inner.finish();
}

fn watch_child(inner: &Inner) {
    while inner.alive.load(Ordering::SeqCst) {
        let exited = match lock(&inner.child).as_mut() {
            None => true,
            Some(child) => child.try_wait().map_or(true, |status| status.is_some()),
        };
        if exited {
            inner.finish();
            return;
        }
        std::thread::sleep(CHILD_POLL);
    }
}

/// An outstanding request. Dropping it before the answer arrives cancels the request.
#[derive(Debug)]
pub struct Pending<T> {
    inner: Arc<Inner>,
    id: i64,
    receiver: mpsc::Receiver<Answer>,
    early: Option<LspError>,
    done: bool,
    _result: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Pending<T> {
    pub fn wait(mut self, timeout: Duration) -> Result<T, LspError> {
        if let Some(error) = self.early.take() {
            return Err(error);
        }
        let answer = match self.receiver.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => return Err(LspError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(LspError::ServerExited),
        };
        self.done = true;
        let value = answer?;
        serde_json::from_value(value).map_err(|error| LspError::Decode(error.to_string()))
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let was_pending = lock(&self.inner.pending).remove(&self.id).is_some();
        if was_pending && self.inner.alive.load(Ordering::SeqCst) {
            let _ = self.inner.send(&json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": {"id": self.id},
            }));
        }
    }
}
