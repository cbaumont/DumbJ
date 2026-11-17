use std::sync::Arc;
use std::time::Duration;

use dumbj_settings::UiRole;
use dumbj_text::{ByteOffset, LineIndex};
use dumbj_vcs::diff::{Hunk, HunkKind, line_hunks, old_text};
use gpui::{AppContext, Context, Task};

use super::{EditorView, Reveal};

/// Typing pauses this long before the change markers are recomputed.
const TYPING_DELAY: Duration = Duration::from_millis(200);

/// How the text differs from the file's content in the HEAD commit.
#[derive(Default)]
pub(super) struct Changes {
    base: Option<Arc<String>>,
    hunks: Arc<Vec<Hunk>>,
    task: Option<Task<()>>,
}

/// What the gutter draws beside a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChangeMark {
    Added,
    Modified,
    /// Lines were removed just above this line.
    DeletedAbove,
    /// Lines were removed after this line, the last one.
    DeletedBelow,
}

impl ChangeMark {
    pub(super) fn role(self) -> UiRole {
        match self {
            Self::Added => UiRole::Success,
            Self::Modified => UiRole::Info,
            Self::DeletedAbove | Self::DeletedBelow => UiRole::Error,
        }
    }
}

fn mark_of(hunks: &[Hunk], line: usize, line_count: usize) -> Option<ChangeMark> {
    hunks.iter().find_map(|hunk| match hunk.kind {
        HunkKind::Added if hunk.new.contains(&line) => Some(ChangeMark::Added),
        HunkKind::Modified if hunk.new.contains(&line) => Some(ChangeMark::Modified),
        HunkKind::Deleted if hunk.new.start == line => Some(ChangeMark::DeletedAbove),
        HunkKind::Deleted if hunk.new.start >= line_count && line + 1 == line_count => {
            Some(ChangeMark::DeletedBelow)
        }
        _ => None,
    })
}

/// The hunk the caret's line belongs to; a deletion belongs to the line below it.
fn hunk_at(hunks: &[Hunk], line: usize, line_count: usize) -> Option<&Hunk> {
    hunks.iter().find(|hunk| {
        hunk.new.contains(&line)
            || (hunk.new.is_empty()
                && (hunk.new.start == line
                    || (hunk.new.start >= line_count && line + 1 == line_count)))
    })
}

fn head_line(hunks: &[Hunk], line: usize) -> Option<usize> {
    let (mut old_end, mut new_end) = (0, 0);
    for hunk in hunks {
        if line < hunk.new.start {
            break;
        }
        if line < hunk.new.end {
            return None;
        }
        (old_end, new_end) = (hunk.old.end, hunk.new.end);
    }
    Some(old_end + (line - new_end))
}

fn neighbour(hunks: &[Hunk], line: usize, forward: bool) -> Option<usize> {
    if forward {
        hunks
            .iter()
            .map(|hunk| hunk.new.start)
            .find(|start| *start > line)
    } else {
        hunks
            .iter()
            .rev()
            .filter(|hunk| !hunk.new.contains(&line))
            .map(|hunk| hunk.new.start)
            .find(|start| *start < line)
    }
}

impl EditorView {
    /// Sets the text the change markers compare against; `None` shows no markers (the
    /// file is not in a repository, or is new).
    pub(crate) fn set_base_text(&mut self, base: Option<Arc<String>>, cx: &mut Context<Self>) {
        if self.changes.base == base {
            return;
        }
        self.changes.base = base;
        self.refresh_changes(Duration::ZERO, cx);
    }

    pub(super) fn changes_edited(&mut self, cx: &mut Context<Self>) {
        self.refresh_changes(TYPING_DELAY, cx);
    }

    fn refresh_changes(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(base) = self.changes.base.clone() else {
            self.changes.hunks = Arc::default();
            self.changes.task = None;
            cx.notify();
            return;
        };
        if !self.is_ready() {
            return;
        }
        self.changes.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(buffer) = this.read_with(cx, |this, _| this.model.buffer().clone()) else {
                return;
            };
            let version = buffer.version();
            let hunks = cx
                .background_spawn(async move { line_hunks(&base, &buffer.to_string()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.model.buffer().version() == version {
                    this.changes.hunks = Arc::new(hunks);
                    cx.notify();
                }
            });
        }));
    }

    /// Recomputes the markers once the file has loaded, if a base arrived before it.
    pub(super) fn changes_loaded(&mut self, cx: &mut Context<Self>) {
        self.refresh_changes(Duration::ZERO, cx);
    }

    /// The line of the HEAD text that `line` is, unless it was added or modified since.
    pub(super) fn head_line(&self, line: usize) -> Option<usize> {
        self.changes.base.as_ref()?;
        head_line(&self.changes.hunks, line)
    }

    pub(super) fn change_mark(&self, line: usize) -> Option<ChangeMark> {
        mark_of(&self.changes.hunks, line, self.model.buffer().line_count())
    }

    pub(super) fn go_to_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let line = self.model.caret_point().line.get();
        let Some(target) = neighbour(&self.changes.hunks, line, forward) else {
            return;
        };
        let last = self.model.buffer().line_count() - 1;
        let offset = self.model.buffer().line_start(LineIndex(target.min(last)));
        self.model.move_to(offset, false);
        self.reveal = Some(Reveal::Center);
        cx.notify();
    }

    /// Puts the changed lines around the caret back as they are in HEAD.
    pub(super) fn rollback_change(&mut self, cx: &mut Context<Self>) {
        let buffer = self.model.buffer();
        let line_count = buffer.line_count();
        let line = self.model.caret_point().line.get();
        let (Some(base), Some(hunk)) = (
            self.changes.base.clone(),
            hunk_at(&self.changes.hunks, line, line_count).cloned(),
        ) else {
            return;
        };
        let start_of = |line: usize| {
            if line < line_count {
                buffer.line_start(LineIndex(line))
            } else {
                buffer.len()
            }
        };
        let range = start_of(hunk.new.start)..start_of(hunk.new.end);
        let mut text = old_text(&base, &hunk);
        let at_end = range.start == buffer.len() && buffer.len() > ByteOffset::ZERO;
        let ends_with_break = buffer
            .text(buffer.prev_char(buffer.len())..buffer.len())
            .is_ok_and(|last| last == "\n");
        if at_end && !ends_with_break && !text.is_empty() {
            text.insert(0, '\n');
        }
        let caret = range.start;
        let edits = self.model.replace_range(range, &text);
        self.model.seal_history();
        self.model.move_to(caret, false);
        self.edited(&edits, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks() -> Vec<Hunk> {
        vec![
            Hunk {
                kind: HunkKind::Added,
                old: 1..1,
                new: 1..3,
            },
            Hunk {
                kind: HunkKind::Deleted,
                old: 4..6,
                new: 6..6,
            },
            Hunk {
                kind: HunkKind::Modified,
                old: 8..9,
                new: 8..10,
            },
            Hunk {
                kind: HunkKind::Deleted,
                old: 12..13,
                new: 12..12,
            },
        ]
    }

    #[test]
    fn marks_follow_the_hunks() {
        let hunks = hunks();
        let marks: Vec<_> = (0..12).map(|line| mark_of(&hunks, line, 12)).collect();
        assert_eq!(marks[0], None);
        assert_eq!(marks[1], Some(ChangeMark::Added));
        assert_eq!(marks[2], Some(ChangeMark::Added));
        assert_eq!(marks[3], None);
        assert_eq!(marks[6], Some(ChangeMark::DeletedAbove));
        assert_eq!(marks[9], Some(ChangeMark::Modified));
        assert_eq!(marks[11], Some(ChangeMark::DeletedBelow));
    }

    #[test]
    fn next_and_previous_change() {
        let hunks = hunks();
        assert_eq!(neighbour(&hunks, 0, true), Some(1));
        assert_eq!(neighbour(&hunks, 1, true), Some(6));
        assert_eq!(neighbour(&hunks, 12, true), None);
        assert_eq!(neighbour(&hunks, 9, false), Some(6));
        assert_eq!(neighbour(&hunks, 7, false), Some(6));
        assert_eq!(neighbour(&hunks, 1, false), None);
    }

    #[test]
    fn working_lines_map_back_to_head_lines() {
        let hunks = hunks();
        assert_eq!(head_line(&hunks, 0), Some(0));
        assert_eq!(head_line(&hunks, 1), None);
        assert_eq!(head_line(&hunks, 3), Some(1));
        assert_eq!(head_line(&hunks, 6), Some(6));
        assert_eq!(head_line(&hunks, 8), None);
        assert_eq!(head_line(&hunks, 10), Some(9));
        assert_eq!(head_line(&hunks, 12), Some(13));
    }

    #[test]
    fn the_caret_finds_its_hunk() {
        let hunks = hunks();
        assert_eq!(
            hunk_at(&hunks, 2, 12).map(|hunk| hunk.kind),
            Some(HunkKind::Added)
        );
        assert_eq!(
            hunk_at(&hunks, 6, 12).map(|hunk| hunk.old.clone()),
            Some(4..6)
        );
        assert_eq!(
            hunk_at(&hunks, 11, 12).map(|hunk| hunk.old.clone()),
            Some(12..13)
        );
        assert!(hunk_at(&hunks, 0, 12).is_none());
    }
}
