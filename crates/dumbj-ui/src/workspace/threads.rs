//! A pull request's review threads, shown as blocks inside the diff.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use dumbj_actions::Action;
use dumbj_review::{DiffSide, PullRequestDetails, RepoId, ReviewThread};
use dumbj_settings::NoteSide;
use gpui::{AppContext, Context, Entity, Task};

use super::Workspace;
use super::pulls::connect;
use crate::editor::{Block, BlockRow, BlockTone, EditorView};

const WRAP_COLUMNS: usize = 100;
const SUMMARY_CHARS: usize = 80;

pub(super) struct PullThreads {
    pub(super) details: Option<PullRequestDetails>,
    pub(super) threads: Vec<ReviewThread>,
    /// Threads whose fold state the user flipped from the default.
    toggled: HashSet<String>,
    _task: Option<Task<()>>,
}

/// Breaks `text` into lines of at most `width` characters, at spaces where it can.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        let mut length = 0;
        for word in paragraph.split(' ') {
            let mut word = word;
            let mut size = word.chars().count();
            if length > 0 && length + 1 + size > width {
                lines.push(std::mem::take(&mut line));
                length = 0;
            }
            while size > width {
                let cut = word
                    .char_indices()
                    .nth(width)
                    .map_or(word.len(), |(at, _)| at);
                lines.push(word[..cut].to_owned());
                word = &word[cut..];
                size -= width;
            }
            if length > 0 {
                line.push(' ');
                length += 1;
            }
            line.push_str(word);
            length += size;
        }
        lines.push(line);
    }
    lines
}

fn state(thread: &ReviewThread) -> String {
    let pending = thread.comments.iter().any(|comment| comment.pending);
    let mut state = String::new();
    for (set, label) in [
        (thread.is_resolved, "resolved"),
        (thread.is_outdated, "outdated"),
        (pending, "pending"),
    ] {
        if set {
            state.push_str(" · ");
            state.push_str(label);
        }
    }
    state
}

fn expanded_by_default(thread: &ReviewThread) -> bool {
    !thread.is_resolved && !thread.is_outdated
}

/// The zero-based line a thread sits below; an outdated thread stays where it was written.
fn thread_line(thread: &ReviewThread) -> Option<usize> {
    let line = thread.line.or(thread.original_line)?;
    usize::try_from(line).ok()?.checked_sub(1)
}

fn thread_block(id: usize, thread: &ReviewThread, expanded: bool) -> Option<Block> {
    let line = thread_line(thread)?;
    let first = thread.comments.first()?;
    let row = |text: String, tone| BlockRow { text, tone };
    let mut rows = Vec::new();
    if expanded {
        for (index, comment) in thread.comments.iter().enumerate() {
            let marker = if index == 0 { "▾" } else { " " };
            let day: String = comment.created_at.chars().take(10).collect();
            let pending = if comment.pending { " · pending" } else { "" };
            let state = if index == 0 {
                state(thread)
            } else {
                String::new()
            };
            let state = state.replace(" · pending", "");
            rows.push(row(
                format!("{marker} {} · {day}{pending}{state}", comment.author),
                BlockTone::Title,
            ));
            for text in wrap(&comment.body, WRAP_COLUMNS) {
                rows.push(row(format!("  {text}"), BlockTone::Text));
            }
        }
    } else {
        let summary: String = first
            .body
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(SUMMARY_CHARS)
            .collect();
        let more = match thread.comments.len() {
            1 => String::new(),
            count => format!(" · {count} comments"),
        };
        rows.push(row(
            format!("▸ {}: {summary}{more}{}", first.author, state(thread)),
            BlockTone::Faint,
        ));
    }
    Some(Block { id, line, rows })
}

fn side_of(side: DiffSide) -> NoteSide {
    match side {
        DiffSide::Left => NoteSide::Base,
        DiffSide::Right => NoteSide::Head,
    }
}

impl PullThreads {
    fn expanded(&self, thread: &ReviewThread) -> bool {
        expanded_by_default(thread) != self.toggled.contains(&thread.id)
    }

    fn blocks(&self, path: &Path, side: NoteSide) -> Vec<Block> {
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| Path::new(&thread.path) == path && side_of(thread.side) == side)
            .filter_map(|(id, thread)| thread_block(id, thread, self.expanded(thread)))
            .collect()
    }
}

impl Workspace {
    /// Reads the pull request under review and its threads from GitHub again.
    pub(super) fn load_pull_threads(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(pull)) = (self.repository.clone(), self.review_pull().cloned())
        else {
            self.pull_threads = None;
            self.apply_thread_blocks(cx);
            return;
        };
        let known = self.github.clone();
        let task = cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let github = match known {
                        Some(github) => github,
                        None => Arc::new(connect(&repository)?),
                    };
                    let repo = RepoId {
                        host: pull.host,
                        owner: pull.owner,
                        name: pull.name,
                    };
                    let error = |error: dumbj_review::ReviewError| error.to_string();
                    let details = github
                        .client
                        .pull_request(&repo, pull.number)
                        .map_err(error)?;
                    let threads = github
                        .client
                        .review_threads(&repo, pull.number)
                        .map_err(error)?;
                    Ok::<_, String>((github, details, threads))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok((github, details, threads)) => {
                        this.github = Some(github);
                        if let Some(pull) = this.pull_threads.as_mut() {
                            pull.details = Some(details);
                            pull.threads = threads;
                        }
                    }
                    Err(message) => {
                        tracing::warn!(%message, "cannot read the review threads");
                        this.status_message = Some(format!("GitHub: {message}"));
                    }
                }
                this.apply_thread_blocks(cx);
                cx.notify();
            });
        });
        let previous = self.pull_threads.take();
        self.pull_threads = Some(PullThreads {
            details: previous.as_ref().and_then(|pull| pull.details.clone()),
            threads: previous
                .as_ref()
                .map_or_else(Vec::new, |pull| pull.threads.clone()),
            toggled: previous.map_or_else(HashSet::new, |pull| pull.toggled),
            _task: Some(task),
        });
    }

    fn thread_sites(&self) -> Vec<(Entity<EditorView>, std::path::PathBuf, NoteSide)> {
        let Some(workdir) = self.repository.as_ref().map(|r| r.workdir().to_path_buf()) else {
            return Vec::new();
        };
        self.tabs
            .iter()
            .filter_map(|tab| {
                let diff = tab.diff.as_ref().filter(|diff| self.diff_in_review(diff))?;
                let path = tab.path.strip_prefix(&workdir).ok()?.to_path_buf();
                Some([
                    (diff.left().clone(), path.clone(), NoteSide::Base),
                    (tab.editor.clone(), path, NoteSide::Head),
                ])
            })
            .flatten()
            .collect()
    }

    /// Gives every open diff of the review the blocks of its threads.
    pub(super) fn apply_thread_blocks(&self, cx: &mut Context<Self>) {
        for (editor, path, side) in self.thread_sites() {
            let blocks = self
                .pull_threads
                .as_ref()
                .map_or_else(Vec::new, |pull| pull.blocks(&path, side));
            editor.update(cx, |editor, cx| editor.set_blocks(blocks, cx));
        }
    }

    pub(super) fn toggle_thread(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(pull) = self.pull_threads.as_mut() else {
            return;
        };
        let Some(thread) = pull.threads.get(id) else {
            return;
        };
        if !pull.toggled.remove(&thread.id) {
            pull.toggled.insert(thread.id.clone());
        }
        self.apply_thread_blocks(cx);
    }

    /// The threads of the side the caret is in, as (thread index, zero-based line), by line.
    fn threads_here(&self) -> Vec<(usize, usize)> {
        let (Some(pull), Some(editor)) = (&self.pull_threads, self.active_editor()) else {
            return Vec::new();
        };
        let here = self
            .thread_sites()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == editor);
        let Some((_, path, side)) = here else {
            return Vec::new();
        };
        let mut found: Vec<(usize, usize)> = pull
            .threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| Path::new(&thread.path) == path && side_of(thread.side) == side)
            .filter_map(|(id, thread)| Some((id, thread_line(thread)?)))
            .collect();
        found.sort_by_key(|(_, line)| *line);
        found
    }

    pub(super) fn has_threads_here(&self) -> bool {
        !self.threads_here().is_empty()
    }

    pub(super) fn thread_action(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let threads = self.threads_here();
        let caret = editor.read(cx).caret_line_column().0.saturating_sub(1);
        let target = match action {
            Action::NextThread => threads.iter().find(|(_, line)| *line > caret),
            Action::PreviousThread => threads.iter().rev().find(|(_, line)| *line < caret),
            _ => threads.iter().find(|(_, line)| *line == caret),
        };
        let Some(&(id, line)) = target else {
            return;
        };
        if action == Action::ToggleThread {
            self.toggle_thread(id, cx);
        } else {
            editor.update(cx, |editor, cx| editor.go_to(line + 1, None, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use dumbj_review::ReviewComment;

    use super::*;

    fn comment(author: &str, body: &str, pending: bool) -> ReviewComment {
        ReviewComment {
            id: String::new(),
            author: author.to_owned(),
            body: body.to_owned(),
            created_at: "2026-11-16T09:30:00Z".to_owned(),
            url: String::new(),
            pending,
        }
    }

    fn thread(line: Option<u32>, comments: Vec<ReviewComment>) -> ReviewThread {
        ReviewThread {
            id: "T1".to_owned(),
            path: "src/A.kt".to_owned(),
            side: DiffSide::Right,
            line,
            start_line: None,
            original_line: Some(9),
            original_start_line: None,
            is_resolved: false,
            is_outdated: false,
            can_resolve: true,
            can_unresolve: false,
            can_reply: true,
            comments,
        }
    }

    fn texts(block: &Block) -> Vec<&str> {
        block.rows.iter().map(|row| row.text.as_str()).collect()
    }

    #[test]
    fn text_wraps_at_spaces_and_keeps_paragraphs() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert_eq!(wrap("héllo wörld", 5), ["héllo", "wörld"]);
        assert_eq!(wrap("", 5), [] as [&str; 0]);
    }

    #[test]
    fn an_expanded_thread_lists_every_comment() {
        let thread = thread(
            Some(4),
            vec![
                comment("ana", "Why nullable?\nSee the caller.", false),
                comment("bo", "Fixed.", true),
            ],
        );
        let block = thread_block(7, &thread, true).unwrap();
        assert_eq!((block.id, block.line), (7, 3));
        assert_eq!(
            texts(&block),
            [
                "▾ ana · 2026-11-16",
                "  Why nullable?",
                "  See the caller.",
                "  bo · 2026-11-16 · pending",
                "  Fixed.",
            ]
        );
    }

    #[test]
    fn a_folded_thread_is_one_line_with_its_state() {
        let mut thread = thread(None, vec![comment("ana", "Rename this", false)]);
        thread.is_outdated = true;
        thread.is_resolved = true;
        let block = thread_block(0, &thread, false).unwrap();
        assert_eq!(
            block.line, 8,
            "an outdated thread stays on its original line"
        );
        assert_eq!(texts(&block), ["▸ ana: Rename this · resolved · outdated"]);
        thread.comments.push(comment("bo", "ok", false));
        let block = thread_block(0, &thread, false).unwrap();
        assert_eq!(
            texts(&block),
            ["▸ ana: Rename this · 2 comments · resolved · outdated"]
        );
    }

    #[test]
    fn threads_without_a_place_or_comments_have_no_block() {
        let mut nowhere = thread(None, vec![comment("ana", "x", false)]);
        nowhere.original_line = None;
        assert_eq!(thread_block(0, &nowhere, true), None);
        assert_eq!(thread_block(0, &thread(Some(1), Vec::new()), true), None);
    }

    #[test]
    fn open_threads_start_expanded_and_a_toggle_flips_them() {
        let open = thread(Some(1), vec![comment("ana", "x", false)]);
        let mut resolved = open.clone();
        resolved.id = "T2".to_owned();
        resolved.is_resolved = true;
        let mut pull = PullThreads {
            details: None,
            threads: vec![open.clone(), resolved.clone()],
            toggled: HashSet::new(),
            _task: None,
        };
        assert!(pull.expanded(&open));
        assert!(!pull.expanded(&resolved));
        pull.toggled.insert("T2".to_owned());
        assert!(pull.expanded(&resolved));
        let blocks = pull.blocks(Path::new("src/A.kt"), NoteSide::Head);
        assert_eq!(blocks.len(), 2);
        assert_eq!(pull.blocks(Path::new("src/A.kt"), NoteSide::Base), []);
        assert_eq!(pull.blocks(Path::new("src/B.kt"), NoteSide::Head), []);
    }
}
