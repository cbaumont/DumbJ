use std::ops::Range;

use dumbj_text::is_word_char;

/// A caret movement within one line of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineMotion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Start,
    End,
}

/// Single-line editable text. Offsets are UTF-8 byte offsets on `char` boundaries.
#[derive(Debug, Default, Clone)]
pub(crate) struct LineEdit {
    text: String,
    anchor: usize,
    caret: usize,
    marked: Option<Range<usize>>,
}

pub(crate) fn utf16_to_byte(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (index, c) in text.char_indices() {
        if seen >= units {
            return index;
        }
        seen += c.len_utf16();
    }
    text.len()
}

impl LineEdit {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn caret(&self) -> usize {
        self.caret
    }

    pub(crate) fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub(crate) fn is_reversed(&self) -> bool {
        self.caret < self.anchor
    }

    pub(crate) fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    pub(crate) fn unmark(&mut self) {
        self.marked = None;
    }

    fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp_range(&self, range: &Range<usize>) -> Range<usize> {
        let (start, end) = (self.clamp(range.start), self.clamp(range.end));
        start.min(end)..start.max(end)
    }

    pub(crate) fn set_text(&mut self, text: &str, select_all: bool) {
        self.text = text.replace(['\n', '\r'], " ");
        self.caret = self.text.len();
        self.anchor = if select_all { 0 } else { self.caret };
        self.marked = None;
    }

    pub(crate) fn select(&mut self, range: Range<usize>) {
        let range = self.clamp_range(&range);
        (self.anchor, self.caret) = (range.start, range.end);
    }

    pub(crate) fn select_all(&mut self) {
        (self.anchor, self.caret) = (0, self.text.len());
    }

    pub(crate) fn move_to(&mut self, offset: usize, extend: bool) {
        self.caret = self.clamp(offset);
        if !extend {
            self.anchor = self.caret;
        }
    }

    fn target(&self, motion: LineMotion) -> usize {
        let before = || {
            self.text
                .get(..self.caret)
                .unwrap_or("")
                .char_indices()
                .rev()
        };
        let after = || self.text.get(self.caret..).unwrap_or("").char_indices();
        match motion {
            LineMotion::Left => before().next().map_or(0, |(index, _)| index),
            LineMotion::Right => after()
                .nth(1)
                .map_or(self.text.len(), |(i, _)| self.caret + i),
            LineMotion::Start => 0,
            LineMotion::End => self.text.len(),
            LineMotion::WordLeft => before()
                .skip_while(|(_, c)| !is_word_char(*c))
                .take_while(|(_, c)| is_word_char(*c))
                .last()
                .map_or(0, |(index, _)| index),
            LineMotion::WordRight => after()
                .skip_while(|(_, c)| !is_word_char(*c))
                .find(|(_, c)| !is_word_char(*c))
                .map_or(self.text.len(), |(index, _)| self.caret + index),
        }
    }

    /// Moves the caret. Without `extend`, a horizontal move collapses a selection to its
    /// near edge instead of moving past it.
    pub(crate) fn apply_motion(&mut self, motion: LineMotion, extend: bool) {
        let selection = self.selection();
        let target = match motion {
            LineMotion::Left if !extend && !selection.is_empty() => selection.start,
            LineMotion::Right if !extend && !selection.is_empty() => selection.end,
            _ => self.target(motion),
        };
        self.move_to(target, extend);
    }

    /// Replaces `range` (or the composition, or the selection) with `text` and puts the
    /// caret after it. Line breaks become spaces. Ends any composition.
    pub(crate) fn replace(&mut self, range: Option<Range<usize>>, text: &str) -> Range<usize> {
        let range = range
            .map(|range| self.clamp_range(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let text = text.replace(['\n', '\r'], " ");
        self.text.replace_range(range.clone(), &text);
        self.caret = range.start + text.len();
        self.anchor = self.caret;
        self.marked = None;
        range.start..self.caret
    }

    /// Like [`replace`](Self::replace), but the inserted text stays marked as an in-progress
    /// composition. `selected` is relative to the start of the inserted text.
    pub(crate) fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let inserted = self.replace(range, text);
        self.marked = (!inserted.is_empty()).then(|| inserted.clone());
        if let Some(selected) = selected {
            self.select(inserted.start + selected.start..inserted.start + selected.end);
        }
    }

    /// Deletes the selection, or the character before the caret (`forward == false`) or
    /// after it. Returns whether anything changed.
    pub(crate) fn delete(&mut self, forward: bool) -> bool {
        if self.selection().is_empty() {
            let motion = if forward {
                LineMotion::Right
            } else {
                LineMotion::Left
            };
            self.caret = self.target(motion);
        }
        let changed = !self.selection().is_empty();
        self.replace(None, "");
        changed
    }

    pub(crate) fn selected_text(&self) -> &str {
        self.text.get(self.selection()).unwrap_or("")
    }

    pub(crate) fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text
            .get(..self.clamp(offset))
            .map_or(0, |prefix| prefix.encode_utf16().count())
    }

    pub(crate) fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub(crate) fn utf16_to_range(&self, range: &Range<usize>) -> Range<usize> {
        utf16_to_byte(&self.text, range.start)..utf16_to_byte(&self.text, range.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str) -> LineEdit {
        let mut edit = LineEdit::default();
        edit.set_text(text, false);
        edit
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = edit("hello world");
        e.select(0..5);
        e.replace(None, "bye");
        assert_eq!(e.text(), "bye world");
        assert_eq!(e.selection(), 3..3);
        e.replace(None, "\nnow\r");
        assert_eq!(e.text(), "bye now  world", "line breaks become spaces");
    }

    #[test]
    fn set_text_can_select_everything() {
        let mut e = LineEdit::default();
        e.set_text("12:3", true);
        assert_eq!(e.selection(), 0..4);
        assert_eq!(e.selected_text(), "12:3");
        e.replace(None, "7");
        assert_eq!(e.text(), "7");
    }

    #[test]
    fn motions_respect_multibyte_characters_and_words() {
        let mut e = edit("añ foo_bar  baz");
        e.apply_motion(LineMotion::Start, false);
        e.apply_motion(LineMotion::Right, false);
        e.apply_motion(LineMotion::Right, false);
        assert_eq!(e.caret(), 3, "ñ is two bytes");
        e.apply_motion(LineMotion::Left, false);
        assert_eq!(e.caret(), 1);
        e.apply_motion(LineMotion::WordRight, false);
        assert_eq!(e.caret(), 3);
        e.apply_motion(LineMotion::WordRight, false);
        assert_eq!(e.caret(), 11, "end of foo_bar");
        e.apply_motion(LineMotion::WordRight, false);
        assert_eq!(e.caret(), 16);
        e.apply_motion(LineMotion::WordRight, false);
        assert_eq!(e.caret(), 16, "stays at the end");
        e.apply_motion(LineMotion::WordLeft, false);
        assert_eq!(e.caret(), 13, "start of baz");
        e.apply_motion(LineMotion::WordLeft, false);
        assert_eq!(e.caret(), 4);
        e.apply_motion(LineMotion::Start, false);
        e.apply_motion(LineMotion::Left, false);
        assert_eq!(e.caret(), 0, "stays at the start");
    }

    #[test]
    fn selection_extends_and_collapses() {
        let mut e = edit("abcdef");
        e.move_to(2, false);
        e.apply_motion(LineMotion::Right, true);
        e.apply_motion(LineMotion::Right, true);
        assert_eq!(e.selection(), 2..4);
        assert!(!e.is_reversed());
        e.apply_motion(LineMotion::Left, false);
        assert_eq!(e.selection(), 2..2, "collapses to the near edge");
        e.apply_motion(LineMotion::Start, true);
        assert_eq!(e.selection(), 0..2);
        assert!(e.is_reversed());
        e.apply_motion(LineMotion::Right, false);
        assert_eq!(e.caret(), 2);
        e.select_all();
        assert_eq!(e.selection(), 0..6);
    }

    #[test]
    fn delete_and_backspace() {
        let mut e = edit("añb");
        assert!(e.delete(false));
        assert_eq!(e.text(), "añ");
        assert!(e.delete(false));
        assert_eq!(e.text(), "a");
        e.move_to(0, false);
        assert!(!e.delete(false), "nothing before the caret");
        assert!(e.delete(true));
        assert_eq!(e.text(), "");
        assert!(!e.delete(true));
        let mut e = edit("abcdef");
        e.select(1..4);
        assert!(e.delete(true));
        assert_eq!(e.text(), "aef");
    }

    #[test]
    fn ime_composition_marks_and_commits() {
        let mut e = edit("ab");
        e.move_to(1, false);
        e.replace_and_mark(None, "に", Some(3..3));
        assert_eq!(e.text(), "aにb");
        assert_eq!(e.marked(), Some(1..4));
        e.replace_and_mark(None, "にほ", None);
        assert_eq!(
            e.text(),
            "aにほb",
            "the composition is replaced, not appended"
        );
        assert_eq!(e.marked(), Some(1..7));
        e.replace(None, "日本");
        assert_eq!(e.text(), "a日本b");
        assert_eq!(e.marked(), None);
        assert_eq!(e.caret(), 7);
        e.replace_and_mark(None, "", None);
        assert_eq!(e.marked(), None, "an empty composition is not marked");
    }

    #[test]
    fn utf16_conversions() {
        let e = edit("a𝄞é");
        assert_eq!(e.offset_to_utf16(1), 1);
        assert_eq!(e.offset_to_utf16(5), 3, "𝄞 is two UTF-16 units");
        assert_eq!(e.offset_to_utf16(7), 4);
        assert_eq!(utf16_to_byte(e.text(), 3), 5);
        assert_eq!(utf16_to_byte(e.text(), 99), 7);
        assert_eq!(e.utf16_to_range(&e.range_to_utf16(&(1..5))), 1..5);
    }

    #[test]
    fn out_of_range_offsets_are_clamped() {
        let mut e = edit("é");
        e.select(1..99);
        assert_eq!(
            e.selection(),
            0..2,
            "inside é snaps back; past the end clamps"
        );
        e.replace(Some(5..9), "x");
        assert_eq!(e.text(), "éx");
    }
}
