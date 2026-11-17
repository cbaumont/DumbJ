use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dumbj_lsp::{Detection, Environment, Server, ServerKind, Session, SessionState, detect};
use gpui::{AppContext, Context, Task};

use super::Workspace;
use super::state::is_project_file;

const SYNC_DELAY: Duration = Duration::from_millis(300);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const TOKENS_TIMEOUT: Duration = Duration::from_secs(10);

/// The project's language servers. A server starts when the first file of its language
/// opens, not with the project, so a Kotlin-only project never pays for jdtls.
#[derive(Default)]
pub(super) struct Smart {
    sessions: HashMap<ServerKind, Session>,
    unavailable: HashMap<ServerKind, String>,
    detecting: HashSet<ServerKind>,
    /// The buffer version each open document was last sent at.
    sent: HashMap<PathBuf, u64>,
    /// The buffer version whose semantic tokens were asked for, per file.
    tokens: HashMap<PathBuf, u64>,
    changed: Arc<AtomicBool>,
    sync: Option<Task<()>>,
    poll: Option<Task<()>>,
    pub(super) nav: Option<super::smart_nav::SmartNav>,
    pub(super) generation: u64,
    /// The hierarchy request whose dumb tree waits for the exact one.
    pub(super) hierarchy_waiting: Option<u64>,
    pub(super) peek: Option<super::peek::Peek>,
    /// The generation of the peek the user closed, so a late exact answer stays away.
    pub(super) peek_closed: u64,
    /// Where the last navigation went; the caret gets there only once the file has loaded.
    pub(super) last_jump: Option<super::navigation::Location>,
}

fn label(kind: ServerKind, state: &SessionState) -> String {
    match state {
        SessionState::Starting => "smart (starting…)".to_owned(),
        SessionState::Indexing(_) => "smart (indexing…)".to_owned(),
        SessionState::Ready => "smart".to_owned(),
        SessionState::Restarting => format!("dumb ({} restarting…)", kind.label()),
        SessionState::Failed(reason) => format!("dumb ({reason})"),
    }
}

/// Where the servers of the project at `root` keep their workspace data:
/// `$XDG_CACHE_HOME/dumbj/lsp/<id>/`, never inside the project.
fn data_dir(root: &Path) -> Option<PathBuf> {
    let cache = dumbj_index::cache_path(root)?;
    let id = cache.file_stem()?;
    Some(cache.parent()?.parent()?.join("lsp").join(id))
}

impl Workspace {
    fn smart_kind(&self, path: &Path) -> Option<ServerKind> {
        let root = &self.project.as_ref()?.root;
        (is_project_file(path) && path.starts_with(root))
            .then(|| ServerKind::for_path(path))
            .flatten()
    }

    pub(super) fn smart_label(&self) -> String {
        let kind = self
            .active_tab()
            .filter(|tab| tab.shows_working_file() && !self.in_revision())
            .and_then(|tab| self.smart_kind(&tab.path));
        let Some(kind) = kind.filter(|_| !self.state.smart_off) else {
            return "dumb".to_owned();
        };
        if let Some(session) = self.smart.sessions.get(&kind) {
            return label(kind, &session.state());
        }
        match self.smart.unavailable.get(&kind) {
            Some(reason) => format!("dumb ({reason})"),
            None => "dumb".to_owned(),
        }
    }

    /// The server that can answer for `path` right now: none while it starts, indexes or
    /// is down, and none for revisions, library sources and files outside the project.
    pub(super) fn smart_server(&self, path: &Path) -> Option<Server> {
        let session = self.smart.sessions.get(&self.smart_kind(path)?)?;
        (session.state() == SessionState::Ready)
            .then(|| session.server())
            .flatten()
    }

    pub(super) fn reset_smart(&mut self, cx: &mut Context<Self>) {
        for session in self.smart.sessions.values() {
            session.stop();
        }
        for tab in &self.tabs {
            tab.editor
                .update(cx, |editor, cx| editor.set_diagnostics(&[], cx));
        }
        self.smart = Smart {
            generation: self.smart.generation,
            ..Smart::default()
        };
    }

    pub(super) fn toggle_smart_mode(&mut self, cx: &mut Context<Self>) {
        self.state.smart_off = !self.state.smart_off;
        self.save_project_state(cx);
        self.reset_smart(cx);
        self.status_message = Some(
            if self.state.smart_off {
                "Smart mode is off for this project"
            } else {
                "Smart mode is on for this project"
            }
            .to_owned(),
        );
        self.schedule_smart_sync(cx);
        cx.notify();
    }

    pub(super) fn smart_state_loaded(&mut self, cx: &mut Context<Self>) {
        if self.state.smart_off {
            self.reset_smart(cx);
            cx.notify();
        }
    }

    fn ensure_session(&mut self, kind: ServerKind, cx: &mut Context<Self>) {
        let smart = &self.smart;
        if smart.sessions.contains_key(&kind)
            || smart.unavailable.contains_key(&kind)
            || smart.detecting.contains(&kind)
        {
            return;
        }
        let Some(root) = self.project.as_ref().map(|project| project.root.clone()) else {
            return;
        };
        self.smart.detecting.insert(kind);
        cx.spawn(async move |this, cx| {
            let lookup = root.clone();
            let detection = cx
                .background_spawn(async move {
                    match data_dir(&lookup) {
                        Some(dir) => detect(kind, &lookup, &dir, &Environment::from_process()),
                        None => Detection::Unavailable("no cache directory".to_owned()),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let same_project = this
                    .project
                    .as_ref()
                    .is_some_and(|project| project.root == root);
                if !this.smart.detecting.remove(&kind) || !same_project || this.state.smart_off {
                    return;
                }
                match detection {
                    Detection::Ready(command) => {
                        let changed = this.smart.changed.clone();
                        let session = Session::start(kind, command, root, move || {
                            changed.store(true, Ordering::Relaxed);
                        });
                        this.smart.sessions.insert(kind, session);
                        this.start_smart_poll(cx);
                        this.sync_documents(cx);
                    }
                    Detection::Unavailable(reason) => {
                        tracing::info!(server = kind.label(), %reason, "staying in dumb mode");
                        this.smart.unavailable.insert(kind, reason);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sessions report changes from their own threads; this turns them into repaints.
    fn start_smart_poll(&mut self, cx: &mut Context<Self>) {
        if self.smart.poll.is_some() {
            return;
        }
        let changed = self.smart.changed.clone();
        self.smart.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if changed.swap(false, Ordering::Relaxed)
                    && this
                        .update(cx, |this, cx| {
                            this.refresh_from_servers(cx);
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(super) fn schedule_smart_sync(&mut self, cx: &mut Context<Self>) {
        if self.state.smart_off || self.project.is_none() {
            return;
        }
        self.smart.sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_DELAY).await;
            let _ = this.update(cx, Self::sync_documents);
        }));
    }

    fn sync_documents(&mut self, cx: &mut Context<Self>) {
        if self.state.smart_off {
            return;
        }
        let mut wanted = Vec::new();
        let mut loading = false;
        for tab in self.tabs.iter().filter(|tab| tab.shows_working_file()) {
            let Some(kind) = self.smart_kind(&tab.path) else {
                continue;
            };
            let editor = tab.editor.read(cx);
            match editor.document_version() {
                Some(version) => wanted.push((kind, tab.path.clone(), version, tab.editor.clone())),
                None => loading = true,
            }
        }
        for (kind, path, version, editor) in wanted {
            self.ensure_session(kind, cx);
            let Some(session) = self.smart.sessions.get(&kind) else {
                continue;
            };
            if self.smart.sent.get(&path) != Some(&version) {
                session.change_document(&path, editor.read(cx).text());
                self.smart.sent.insert(path, version);
            }
        }
        if loading {
            self.schedule_smart_sync(cx);
        }
        self.refresh_from_servers(cx);
    }

    /// Gives each open working file the diagnostics its server last published, and asks
    /// for the semantic tokens of files whose text the server has not coloured yet.
    fn refresh_from_servers(&mut self, cx: &mut Context<Self>) {
        let mut wanted = Vec::new();
        for tab in self.tabs.iter().filter(|tab| tab.shows_working_file()) {
            let session = self
                .smart_kind(&tab.path)
                .and_then(|kind| self.smart.sessions.get(&kind));
            let diagnostics =
                session.map_or_else(Default::default, |session| session.diagnostics(&tab.path));
            tab.editor
                .update(cx, |editor, cx| editor.set_diagnostics(&diagnostics, cx));
            let ready = session.filter(|session| session.state() == SessionState::Ready);
            let Some(session) = ready else {
                self.smart.tokens.remove(&tab.path);
                continue;
            };
            let version = tab.editor.read(cx).document_version();
            if let (Some(version), Some(server), Some(legend)) =
                (version, session.server(), session.semantic_legend())
                && self.smart.sent.get(&tab.path) == Some(&version)
                && self.smart.tokens.get(&tab.path) != Some(&version)
            {
                wanted.push((
                    tab.path.clone(),
                    version,
                    tab.editor.clone(),
                    server,
                    legend,
                ));
            }
        }
        for (path, version, editor, server, legend) in wanted {
            self.smart.tokens.insert(path.clone(), version);
            let tokens = cx.background_spawn(async move {
                dumbj_lsp::semantic_tokens(&server, &path, &legend, TOKENS_TIMEOUT)
            });
            cx.spawn(async move |_, cx| match tokens.await {
                Ok(tokens) => {
                    editor.update(cx, |editor, cx| {
                        editor.set_semantic_tokens(version, &tokens, cx);
                    });
                }
                Err(error) => tracing::debug!(%error, "no semantic tokens"),
            })
            .detach();
        }
    }

    pub(super) fn problem_at_caret(&self, cx: &gpui::App) -> Option<String> {
        let problem = self.active_editor()?.read(cx).problem_at_caret()?;
        let mut message = problem.message.clone();
        if message.chars().count() > 120 {
            message = message.chars().take(119).chain(['…']).collect();
        }
        Some(message)
    }

    pub(super) fn go_to_problem(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        if !editor.update(cx, |editor, cx| editor.go_to_problem(forward, cx)) {
            self.status_message = Some("No problems in this file".to_owned());
            cx.notify();
        }
    }

    pub(super) fn smart_tab_closed(&mut self, path: &Path) {
        if self.tabs.iter().any(|tab| tab.path == path) {
            return;
        }
        if self.smart.sent.remove(path).is_some()
            && let Some(session) = self
                .smart_kind(path)
                .and_then(|kind| self.smart.sessions.get(&kind))
        {
            session.close_document(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_say_which_engine_answers() {
        let kind = ServerKind::Jdtls;
        assert_eq!(label(kind, &SessionState::Ready), "smart");
        assert_eq!(
            label(kind, &SessionState::Indexing(Some("Importing".into()))),
            "smart (indexing…)"
        );
        assert_eq!(
            label(kind, &SessionState::Restarting),
            "dumb (jdtls restarting…)"
        );
        assert_eq!(
            label(kind, &SessionState::Failed("jdtls keeps crashing".into())),
            "dumb (jdtls keeps crashing)"
        );
    }
}
