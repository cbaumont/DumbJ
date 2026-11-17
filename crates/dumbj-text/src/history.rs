use std::collections::VecDeque;

use crate::buffer::{Buffer, Edit, TextError, is_word_char};
use crate::position::{ByteOffset, ByteRange};

const MAX_UNDO_GROUPS: usize = 10_000;

/// Selection ends. `anchor == caret` when there is no selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Carets {
    pub anchor: ByteOffset,
    pub caret: ByteOffset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    DeleteBackward,
    DeleteForward,
    Other,
}

#[derive(Debug, Clone)]
struct Change {
    offset: usize,
    removed: String,
    inserted: String,
}

impl Change {
    fn forward_range(&self) -> ByteRange {
        ByteOffset(self.offset)..ByteOffset(self.offset + self.removed.len())
    }

    fn inverse_range(&self) -> ByteRange {
        ByteOffset(self.offset)..ByteOffset(self.offset + self.inserted.len())
    }
}

#[derive(Debug, Clone)]
struct Group {
    id: u64,
    changes: Vec<Change>,
    before: Carets,
    after: Carets,
    kind: EditKind,
    sealed: bool,
}

/// Undo and redo for a [`Buffer`], grouping typing the way IntelliJ does: consecutive
/// inserts or deletions form one step, split at line breaks and word starts.
#[derive(Debug)]
pub struct History {
    undo: VecDeque<Group>,
    redo: Vec<Group>,
    next_id: u64,
    saved: Option<u64>,
    explicit_group: bool,
    start_new_group: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            next_id: 1,
            saved: Some(0),
            explicit_group: false,
            start_new_group: false,
        }
    }
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn current_id(&self) -> u64 {
        self.undo.back().map_or(0, |group| group.id)
    }

    fn joins_latest(&self, change: &Change, kind: EditKind) -> bool {
        let Some(group) = self.undo.back() else {
            return false;
        };
        if self.explicit_group {
            return !self.start_new_group;
        }
        if group.sealed || group.kind != kind {
            return false;
        }
        let Some(last) = group.changes.last() else {
            return false;
        };
        match kind {
            EditKind::Insert => {
                if change.inserted.contains('\n') || !change.removed.is_empty() {
                    return false;
                }
                let starts_word = change.inserted.chars().next().is_some_and(is_word_char);
                let after_space = last
                    .inserted
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
                last.removed.is_empty()
                    && last.offset + last.inserted.len() == change.offset
                    && !(starts_word && after_space)
            }
            EditKind::DeleteBackward => {
                change.inserted.is_empty()
                    && last.inserted.is_empty()
                    && change.offset + change.removed.len() == last.offset
            }
            EditKind::DeleteForward => {
                change.inserted.is_empty()
                    && last.inserted.is_empty()
                    && change.offset == last.offset
            }
            EditKind::Other => false,
        }
    }

    /// Replaces `range` with `text` in `buffer` and records it so it can be undone.
    /// `before` is the caret state before the edit, `after` the state after it.
    pub fn apply(
        &mut self,
        buffer: &mut Buffer,
        range: ByteRange,
        text: &str,
        kind: EditKind,
        before: Carets,
        after: Carets,
    ) -> Result<Edit, TextError> {
        let removed = buffer.text(range.clone())?;
        let offset = range.start.0;
        let edit = buffer.replace(range, text)?;
        if removed.is_empty() && text.is_empty() {
            return Ok(edit);
        }
        let change = Change {
            offset,
            removed,
            inserted: text.to_owned(),
        };
        self.redo.clear();
        if self.joins_latest(&change, kind) {
            if let Some(group) = self.undo.back_mut() {
                group.changes.push(change);
                group.after = after;
            }
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.undo.push_back(Group {
                id,
                changes: vec![change],
                before,
                after,
                kind: if self.explicit_group {
                    EditKind::Other
                } else {
                    kind
                },
                sealed: false,
            });
            if self.undo.len() > MAX_UNDO_GROUPS
                && let Some(dropped) = self.undo.pop_front()
                && (self.saved == Some(dropped.id) || self.saved == Some(0))
            {
                self.saved = None;
            }
        }
        self.start_new_group = false;
        if !self.explicit_group
            && kind == EditKind::Insert
            && text.contains('\n')
            && let Some(group) = self.undo.back_mut()
        {
            group.sealed = true;
        }
        Ok(edit)
    }

    pub fn seal(&mut self) {
        if let Some(group) = self.undo.back_mut() {
            group.sealed = true;
        }
    }

    pub fn begin_group(&mut self) {
        self.seal();
        self.explicit_group = true;
        self.start_new_group = true;
    }

    pub fn end_group(&mut self) {
        self.explicit_group = false;
        self.start_new_group = false;
        self.seal();
    }

    /// Undoes the latest group. Returns the edits applied to the buffer, in application
    /// order, and the caret state to restore.
    pub fn undo(&mut self, buffer: &mut Buffer) -> Option<(Vec<Edit>, Carets)> {
        self.seal();
        let group = self.undo.pop_back()?;
        let mut edits = Vec::with_capacity(group.changes.len());
        for change in group.changes.iter().rev() {
            edits.push(
                buffer
                    .replace(change.inverse_range(), &change.removed)
                    .ok()?,
            );
        }
        let before = group.before;
        self.redo.push(group);
        Some((edits, before))
    }

    pub fn redo(&mut self, buffer: &mut Buffer) -> Option<(Vec<Edit>, Carets)> {
        self.seal();
        let group = self.redo.pop()?;
        let mut edits = Vec::with_capacity(group.changes.len());
        for change in &group.changes {
            edits.push(
                buffer
                    .replace(change.forward_range(), &change.inserted)
                    .ok()?,
            );
        }
        let after = group.after;
        self.undo.push_back(Group {
            sealed: true,
            ..group
        });
        Some((edits, after))
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn mark_saved(&mut self) {
        self.seal();
        self.saved = Some(self.current_id());
    }

    /// Whether the buffer differs from the last [`mark_saved`](Self::mark_saved) point. A
    /// fresh history is not modified.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.saved != Some(self.current_id())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn at(offset: usize) -> Carets {
        Carets {
            anchor: ByteOffset(offset),
            caret: ByteOffset(offset),
        }
    }

    fn put(
        history: &mut History,
        buffer: &mut Buffer,
        range: (usize, usize),
        text: &str,
        kind: EditKind,
    ) {
        history
            .apply(
                buffer,
                ByteOffset(range.0)..ByteOffset(range.1),
                text,
                kind,
                at(range.0),
                at(range.0 + text.len()),
            )
            .unwrap();
    }

    fn type_str(history: &mut History, buffer: &mut Buffer, text: &str) {
        for c in text.chars() {
            let end = buffer.len().0;
            put(
                history,
                buffer,
                (end, end),
                &c.to_string(),
                EditKind::Insert,
            );
        }
    }

    fn undo_all(history: &mut History, buffer: &mut Buffer) -> usize {
        let mut count = 0;
        while history.undo(buffer).is_some() {
            count += 1;
        }
        count
    }

    #[test]
    fn contiguous_typing_is_one_group() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "abc");
        assert_eq!(undo_all(&mut history, &mut buffer), 1);
        assert_eq!(buffer.to_string(), "");
    }

    #[test]
    fn non_contiguous_insert_starts_a_group() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("xy"));
        put(&mut history, &mut buffer, (0, 0), "a", EditKind::Insert);
        put(&mut history, &mut buffer, (2, 2), "b", EditKind::Insert);
        assert_eq!(undo_all(&mut history, &mut buffer), 2);
        assert_eq!(buffer.to_string(), "xy");
    }

    #[test]
    fn replacing_a_selection_is_not_joined() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("abc"));
        put(&mut history, &mut buffer, (0, 0), "x", EditKind::Insert);
        put(&mut history, &mut buffer, (1, 3), "y", EditKind::Insert);
        assert_eq!(undo_all(&mut history, &mut buffer), 2);
        assert_eq!(buffer.to_string(), "abc");
    }

    #[test]
    fn different_kinds_do_not_join_and_other_never_joins() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("abcdef"));
        put(
            &mut history,
            &mut buffer,
            (5, 6),
            "",
            EditKind::DeleteBackward,
        );
        put(
            &mut history,
            &mut buffer,
            (0, 1),
            "",
            EditKind::DeleteForward,
        );
        put(&mut history, &mut buffer, (0, 0), "x", EditKind::Other);
        put(&mut history, &mut buffer, (1, 1), "y", EditKind::Other);
        assert_eq!(undo_all(&mut history, &mut buffer), 4);
        assert_eq!(buffer.to_string(), "abcdef");
    }

    #[test]
    fn newline_starts_a_group_and_seals_itself() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "ab\ncd");
        let (_, carets) = history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "ab\n");
        assert_eq!(carets, at(3));
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "ab");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "");
        assert!(!history.can_undo());
    }

    #[test]
    fn typing_goes_word_by_word() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "foo bar");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "foo ");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "");
    }

    #[test]
    fn backward_deletes_join_when_contiguous() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("abcdef"));
        put(
            &mut history,
            &mut buffer,
            (5, 6),
            "",
            EditKind::DeleteBackward,
        );
        put(
            &mut history,
            &mut buffer,
            (4, 5),
            "",
            EditKind::DeleteBackward,
        );
        put(
            &mut history,
            &mut buffer,
            (0, 1),
            "",
            EditKind::DeleteBackward,
        );
        assert_eq!(buffer.to_string(), "bcd");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "abcd");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "abcdef");
    }

    #[test]
    fn forward_deletes_join_at_the_same_offset() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("abcdef"));
        put(
            &mut history,
            &mut buffer,
            (1, 2),
            "",
            EditKind::DeleteForward,
        );
        put(
            &mut history,
            &mut buffer,
            (1, 2),
            "",
            EditKind::DeleteForward,
        );
        put(
            &mut history,
            &mut buffer,
            (2, 3),
            "",
            EditKind::DeleteForward,
        );
        assert_eq!(buffer.to_string(), "adf");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "adef");
        history.undo(&mut buffer).unwrap();
        assert_eq!(buffer.to_string(), "abcdef");
    }

    #[test]
    fn seal_splits_a_group() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "ab");
        history.seal();
        type_str(&mut history, &mut buffer, "cd");
        assert_eq!(undo_all(&mut history, &mut buffer), 2);
    }

    #[test]
    fn carets_come_from_first_and_last_edit() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "abc");
        let (_, restored) = history.undo(&mut buffer).unwrap();
        assert_eq!(restored, at(0));
        let (_, restored) = history.redo(&mut buffer).unwrap();
        assert_eq!(restored, at(3));
        assert_eq!(buffer.to_string(), "abc");
    }

    #[test]
    fn explicit_group_holds_every_edit() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("one\ntwo\n"));
        history.begin_group();
        put(&mut history, &mut buffer, (4, 8), "", EditKind::Other);
        put(&mut history, &mut buffer, (0, 0), "two\n", EditKind::Insert);
        put(&mut history, &mut buffer, (0, 0), "\n", EditKind::Insert);
        history.end_group();
        assert_eq!(buffer.to_string(), "\ntwo\none\n");
        let (edits, _) = history.undo(&mut buffer).unwrap();
        assert_eq!(edits.len(), 3);
        assert_eq!(buffer.to_string(), "one\ntwo\n");
        assert!(!history.can_undo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "a");
        history.undo(&mut buffer).unwrap();
        assert!(history.can_redo());
        type_str(&mut history, &mut buffer, "b");
        assert!(!history.can_redo());
        assert!(history.redo(&mut buffer).is_none());
    }

    #[test]
    fn empty_apply_records_nothing() {
        let (mut history, mut buffer) = (History::new(), Buffer::new("a"));
        put(&mut history, &mut buffer, (1, 1), "", EditKind::Other);
        assert!(!history.can_undo());
        assert!(!history.is_modified());
    }

    #[test]
    fn modified_tracks_the_saved_point() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        assert!(!history.is_modified());
        type_str(&mut history, &mut buffer, "a");
        assert!(history.is_modified());
        history.mark_saved();
        assert!(!history.is_modified());
        type_str(&mut history, &mut buffer, "b");
        assert!(history.is_modified());
        history.undo(&mut buffer).unwrap();
        assert!(!history.is_modified());
        history.undo(&mut buffer).unwrap();
        assert!(history.is_modified());
        history.redo(&mut buffer).unwrap();
        assert!(!history.is_modified());
    }

    #[test]
    fn discarded_saved_point_stays_modified() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        type_str(&mut history, &mut buffer, "a");
        history.mark_saved();
        history.undo(&mut buffer).unwrap();
        type_str(&mut history, &mut buffer, "b");
        history.undo(&mut buffer).unwrap();
        assert!(history.is_modified());
        history.mark_saved();
        assert!(!history.is_modified());
    }

    #[test]
    fn the_cap_drops_the_oldest_groups() {
        let (mut history, mut buffer) = (History::new(), Buffer::new(""));
        for _ in 0..MAX_UNDO_GROUPS + 5 {
            put(&mut history, &mut buffer, (0, 0), "x", EditKind::Other);
        }
        assert_eq!(undo_all(&mut history, &mut buffer), MAX_UNDO_GROUPS);
        assert_eq!(buffer.len().0, 5);
        assert!(history.is_modified());
    }

    fn boundary(text: &str, index: usize) -> usize {
        let mut index = index.min(text.len());
        while !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn kind_of(index: u8) -> EditKind {
        match index % 4 {
            0 => EditKind::Insert,
            1 => EditKind::DeleteBackward,
            2 => EditKind::DeleteForward,
            _ => EditKind::Other,
        }
    }

    proptest! {
        #[test]
        fn undo_all_then_redo_all_round_trips(
            initial in "[a \\né𝄞]{0,20}",
            ops in prop::collection::vec(
                (0u8..4, 0usize..40, 0usize..40, "[a \\né𝄞]{0,3}", any::<bool>()),
                0..30,
            ),
        ) {
            let mut model = initial.clone();
            let mut buffer = Buffer::new(&initial);
            let mut history = History::new();
            for (kind, a, b, text, seal) in ops {
                if seal {
                    history.seal();
                }
                let (a, b) = (boundary(&model, a), boundary(&model, b));
                let (start, end) = (a.min(b), a.max(b));
                put(&mut history, &mut buffer, (start, end), &text, kind_of(kind));
                model.replace_range(start..end, &text);
                prop_assert_eq!(buffer.to_string(), model.clone());
            }
            undo_all(&mut history, &mut buffer);
            prop_assert_eq!(buffer.to_string(), initial);
            while history.redo(&mut buffer).is_some() {}
            prop_assert_eq!(buffer.to_string(), model);
        }

        #[test]
        fn interleaved_undo_redo_matches_a_model(
            initial in "[a \\né𝄞]{0,20}",
            ops in prop::collection::vec(
                (0u8..3, 0usize..40, 0usize..40, "[a \\né𝄞]{0,3}"),
                0..40,
            ),
        ) {
            let mut model = initial.clone();
            let mut undo_states: Vec<String> = Vec::new();
            let mut redo_states: Vec<String> = Vec::new();
            let mut buffer = Buffer::new(&initial);
            let mut history = History::new();
            for (op, a, b, text) in ops {
                match op {
                    0 => {
                        let (a, b) = (boundary(&model, a), boundary(&model, b));
                        let (start, end) = (a.min(b), a.max(b));
                        put(&mut history, &mut buffer, (start, end), &text, EditKind::Other);
                        if start != end || !text.is_empty() {
                            undo_states.push(model.clone());
                            redo_states.clear();
                        }
                        model.replace_range(start..end, &text);
                    }
                    1 => {
                        let undone = history.undo(&mut buffer).is_some();
                        prop_assert_eq!(undone, !undo_states.is_empty());
                        if let Some(previous) = undo_states.pop() {
                            redo_states.push(std::mem::replace(&mut model, previous));
                        }
                    }
                    _ => {
                        let redone = history.redo(&mut buffer).is_some();
                        prop_assert_eq!(redone, !redo_states.is_empty());
                        if let Some(next) = redo_states.pop() {
                            undo_states.push(std::mem::replace(&mut model, next));
                        }
                    }
                }
                prop_assert_eq!(buffer.to_string(), model.clone());
                prop_assert_eq!(history.can_undo(), !undo_states.is_empty());
                prop_assert_eq!(history.can_redo(), !redo_states.is_empty());
            }
        }
    }
}
