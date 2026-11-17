use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dumbj_actions::Action;
use dumbj_review::{DiffSide, PullRequestDetails, RepoId, ReviewThread};
use dumbj_settings::{DraftComment, NoteSide};
use gpui::{AppContext, Context, Entity, Task, Window};

use super::Workspace;
use super::pulls::connect;
use crate::editor::{Block, BlockRow, BlockTone, EditorView};

const WRAP_COLUMNS: usize = 100;
const WRAP_RANGE: (usize, usize) = (30, 160);
const SUMMARY_CHARS: usize = 80;

pub(super) struct PullThreads {
    pub(super) details: Option<PullRequestDetails>,
    pub(super) threads: Vec<ReviewThread>,
    viewed: Option<Vec<String>>,
    toggled: HashSet<String>,
    _task: Option<Task<()>>,
}

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

fn body_rows(body: &str, width: usize) -> Vec<BlockRow> {
    let row = |text: String, tone| BlockRow { text, tone };
    let mut rows = Vec::new();
    let mut fence: Option<BlockTone> = None;
    for line in body.lines() {
        let marker = line.trim_start().strip_prefix("```");
        match (fence, marker) {
            (Some(_), Some(_)) => fence = None,
            (Some(tone), None) => rows.push(row(format!("  │ {line}"), tone)),
            (None, Some(info)) if info.trim() == "suggestion" => {
                rows.push(row("  Suggested change:".to_owned(), BlockTone::Faint));
                fence = Some(BlockTone::Suggestion);
            }
            (None, Some(_)) => fence = Some(BlockTone::Code),
            (None, None) => rows.extend(
                wrap(line, width)
                    .into_iter()
                    .map(|text| row(format!("  {text}"), BlockTone::Text)),
            ),
        }
    }
    rows
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

fn thread_block(id: usize, thread: &ReviewThread, expanded: bool, width: usize) -> Option<Block> {
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
            rows.extend(body_rows(&comment.body, width));
        }
    } else {
        let summary: String = body_rows(&first.body, SUMMARY_CHARS)
            .first()
            .map_or("", |row| row.text.trim_start_matches([' ', '│']))
            .trim_end_matches(':')
            .to_owned();
        let more = match thread.comments.len() {
            1 => String::new(),
            count => format!(" · {count} comments"),
        };
        rows.push(row(
            format!("▸ {}{}{more}: {summary}", first.author, state(thread)),
            BlockTone::Faint,
        ));
    }
    Some(Block { id, line, rows })
}

/// Block ids from here on stand for unsent comments: this plus the draft's id.
const DRAFT_BLOCKS: usize = usize::MAX / 2;

fn draft_block(draft: &DraftComment, width: usize) -> Block {
    let row = |text: String, tone| BlockRow { text, tone };
    let mut rows = vec![row(
        "▾ not sent yet · click to edit".to_owned(),
        BlockTone::Title,
    )];
    rows.extend(body_rows(&draft.body, width));
    Block {
        id: DRAFT_BLOCKS.saturating_add(usize::try_from(draft.id).unwrap_or(0)),
        line: draft.last_line.saturating_sub(1),
        rows,
    }
}

fn wrap_width(editor: &Entity<EditorView>, cx: &gpui::App) -> usize {
    editor
        .read(cx)
        .text_columns()
        .map_or(WRAP_COLUMNS, |columns| {
            columns.saturating_sub(4).clamp(WRAP_RANGE.0, WRAP_RANGE.1)
        })
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

    fn blocks(&self, path: &Path, side: NoteSide, width: usize) -> Vec<Block> {
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| Path::new(&thread.path) == path && side_of(thread.side) == side)
            .filter_map(|(id, thread)| thread_block(id, thread, self.expanded(thread), width))
            .collect()
    }
}

impl Workspace {
    pub(super) fn load_pull_threads(&mut self, cx: &mut Context<Self>) {
        let (Some(repository), Some(pull)) = (self.repository.clone(), self.review_pull().cloned())
        else {
            self.session.pull_threads = None;
            self.apply_thread_blocks(cx);
            return;
        };
        let known = self.session.github.clone();
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
                    let viewed = github
                        .client
                        .viewed_files(&repo, pull.number)
                        .map_err(error)?;
                    Ok::<_, String>((github, details, threads, viewed))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok((github, details, threads, viewed)) => {
                        this.session.github = Some(github);
                        if let Some(pull) = this.session.pull_threads.as_mut() {
                            pull.details = Some(details);
                            pull.threads = threads;
                            pull.viewed = Some(viewed);
                        }
                        this.take_viewed_from_github(cx);
                    }
                    Err(message) => {
                        tracing::warn!(%message, "cannot read the review threads");
                        this.status_message = Some(format!("GitHub: {message}"));
                    }
                }
                this.apply_thread_blocks(cx);
                this.send_drafts(cx);
                cx.notify();
            });
        });
        let previous = self.session.pull_threads.take();
        self.session.pull_threads = Some(PullThreads {
            details: previous.as_ref().and_then(|pull| pull.details.clone()),
            threads: previous
                .as_ref()
                .map_or_else(Vec::new, |pull| pull.threads.clone()),
            toggled: previous.map_or_else(HashSet::new, |pull| pull.toggled),
            viewed: None,
            _task: Some(task),
        });
    }

    fn thread_sites(&self) -> Vec<(Entity<EditorView>, PathBuf, NoteSide)> {
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

    pub(super) fn rewrap_threads(&mut self, cx: &mut Context<Self>) {
        if self.session.pull_threads.is_none() {
            return;
        }
        let widths: Vec<usize> = self
            .thread_sites()
            .iter()
            .map(|(editor, ..)| wrap_width(editor, cx))
            .collect();
        if widths != self.session.wrapped_at {
            self.session.wrapped_at = widths;
            self.apply_thread_blocks(cx);
        }
    }

    pub(super) fn apply_thread_blocks(&self, cx: &mut Context<Self>) {
        let drafts = self
            .state
            .review
            .as_ref()
            .filter(|review| review.pull.is_some())
            .map_or(&[][..], |review| review.drafts());
        for (editor, path, side) in self.thread_sites() {
            let width = wrap_width(&editor, cx);
            let mut blocks = self
                .session
                .pull_threads
                .as_ref()
                .map_or_else(Vec::new, |pull| pull.blocks(&path, side, width));
            blocks.extend(
                drafts
                    .iter()
                    .filter(|draft| draft.path == path && draft.side == side)
                    .map(|draft| draft_block(draft, width)),
            );
            editor.update(cx, |editor, cx| editor.set_blocks(blocks, cx));
        }
    }

    pub(super) fn github_viewed(&mut self) -> Option<Vec<String>> {
        self.session.pull_threads.as_mut()?.viewed.take()
    }

    pub(super) fn push_viewed(&mut self, path: &Path, viewed: bool, cx: &mut Context<Self>) {
        let details = self
            .session
            .pull_threads
            .as_ref()
            .and_then(|pull| pull.details.as_ref());
        let (Some(pull_id), Some(github)) = (
            details.map(|details| details.id.clone()),
            self.session.github.clone(),
        ) else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    if viewed {
                        github.client.mark_file_viewed(&pull_id, &path)
                    } else {
                        github.client.unmark_file_viewed(&pull_id, &path)
                    }
                })
                .await;
            if let Err(error) = outcome {
                let _ = this.update(cx, |this, cx| {
                    this.status_message = Some(format!("Viewed mark not synced: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn reload_pull_threads(&mut self, cx: &mut Context<Self>) {
        self.load_pull_threads(cx);
        cx.notify();
    }

    pub(super) fn block_clicked(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        match id.checked_sub(DRAFT_BLOCKS) {
            Some(draft) => self.compose_draft(draft as u64, window, cx),
            None => self.toggle_thread(id, cx),
        }
    }

    fn set_resolved(&mut self, id: usize, cx: &mut Context<Self>) {
        let thread = self
            .session
            .pull_threads
            .as_ref()
            .and_then(|pull| pull.threads.get(id));
        let (Some(thread), Some(github)) = (thread, self.session.github.clone()) else {
            return;
        };
        let (thread_id, resolve) = (thread.id.clone(), !thread.is_resolved);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    if resolve {
                        github.client.resolve_thread(&thread_id)
                    } else {
                        github.client.unresolve_thread(&thread_id)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = outcome {
                    this.status_message = Some(format!("GitHub: {error}"));
                }
                this.reload_pull_threads(cx);
            });
        })
        .detach();
    }

    pub(super) fn toggle_thread(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(pull) = self.session.pull_threads.as_mut() else {
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

    fn threads_here(&self) -> Vec<(usize, usize)> {
        let (Some(pull), Some(editor)) = (&self.session.pull_threads, self.active_editor()) else {
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

    pub(super) fn thread_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        match action {
            Action::ToggleThread => self.toggle_thread(id, cx),
            Action::ResolveThread => self.set_resolved(id, cx),
            Action::ReplyToThread => {
                let thread = self
                    .session
                    .pull_threads
                    .as_ref()
                    .and_then(|pull| pull.threads.get(id));
                if let Some(thread) = thread {
                    let (thread_id, path, side) = (
                        thread.id.clone(),
                        PathBuf::from(&thread.path),
                        side_of(thread.side),
                    );
                    self.compose_reply(thread_id, path, side, line + 1, window, cx);
                }
            }
            _ => editor.update(cx, |editor, cx| editor.go_to(line + 1, None, cx)),
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
    fn code_fences_are_kept_and_suggestions_stand_out() {
        let body = "Use the size once, it is computed twice here.\n```suggestion\n    val size = tracks.size\n```\nOr:\n```kotlin\nfoo()\n```";
        let rows = body_rows(body, 20);
        let rows: Vec<(&str, BlockTone)> = rows
            .iter()
            .map(|row| (row.text.as_str(), row.tone))
            .collect();
        assert_eq!(
            rows,
            [
                ("  Use the size once,", BlockTone::Text),
                ("  it is computed twice", BlockTone::Text),
                ("  here.", BlockTone::Text),
                ("  Suggested change:", BlockTone::Faint),
                ("  │     val size = tracks.size", BlockTone::Suggestion),
                ("  Or:", BlockTone::Text),
                ("  │ foo()", BlockTone::Code),
            ]
        );
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
        let block = thread_block(7, &thread, true, 100).unwrap();
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
        let block = thread_block(0, &thread, false, 100).unwrap();
        assert_eq!(
            block.line, 8,
            "an outdated thread stays on its original line"
        );
        assert_eq!(texts(&block), ["▸ ana · resolved · outdated: Rename this"]);
        thread.comments.push(comment("bo", "ok", false));
        let block = thread_block(0, &thread, false, 100).unwrap();
        assert_eq!(
            texts(&block),
            ["▸ ana · resolved · outdated · 2 comments: Rename this"]
        );
    }

    #[test]
    fn threads_without_a_place_or_comments_have_no_block() {
        let mut nowhere = thread(None, vec![comment("ana", "x", false)]);
        nowhere.original_line = None;
        assert_eq!(thread_block(0, &nowhere, true, 100), None);
        assert_eq!(
            thread_block(0, &thread(Some(1), Vec::new()), true, 100),
            None
        );
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
            viewed: None,
            _task: None,
        };
        assert!(pull.expanded(&open));
        assert!(!pull.expanded(&resolved));
        pull.toggled.insert("T2".to_owned());
        assert!(pull.expanded(&resolved));
        let blocks = pull.blocks(Path::new("src/A.kt"), NoteSide::Head, 100);
        assert_eq!(blocks.len(), 2);
        assert_eq!(pull.blocks(Path::new("src/A.kt"), NoteSide::Base, 100), []);
        assert_eq!(pull.blocks(Path::new("src/B.kt"), NoteSide::Head, 100), []);
    }
}
