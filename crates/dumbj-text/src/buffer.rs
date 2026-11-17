use std::borrow::Cow;

use ropey::{Rope, RopeSlice};
use thiserror::Error;

use crate::position::{ByteOffset, ByteRange, LineIndex, Point};

#[must_use]
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TextError {
    #[error("range {start}..{end} is outside the buffer (length {len}) or reversed")]
    RangeOutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("offset {0} is not on a character boundary")]
    NotCharBoundary(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    pub start: ByteOffset,
    pub old_end: ByteOffset,
    pub new_end: ByteOffset,
    pub start_point: Point,
    pub old_end_point: Point,
    pub new_end_point: Point,
}

/// A text buffer. Lines are separated by `\n`; a `\r` before it is kept as part of the text
/// but never counted as line content.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    rope: Rope,
    version: u64,
}

impl Buffer {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            version: 0,
        }
    }

    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn len(&self) -> ByteOffset {
        ByteOffset(self.rope.len_bytes())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// The number of lines. An empty buffer has one (empty) line, and a trailing newline
    /// starts a final empty line.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    fn last_line(&self) -> LineIndex {
        LineIndex(self.line_count() - 1)
    }

    fn line_slice(&self, line: LineIndex) -> RopeSlice<'_> {
        let slice = self.rope.line(line.0.min(self.line_count() - 1));
        let mut len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
            if len > 0 && slice.char(len - 1) == '\r' {
                len -= 1;
            }
        }
        slice.slice(..len)
    }

    /// The text of a line without its line break. Lines past the end clamp to the last line.
    #[must_use]
    pub fn line(&self, line: LineIndex) -> Cow<'_, str> {
        self.line_slice(line).into()
    }

    #[must_use]
    pub fn line_len(&self, line: LineIndex) -> usize {
        self.line_slice(line).len_bytes()
    }

    /// The offset of the first byte of a line. Lines past the end clamp to the last line.
    #[must_use]
    pub fn line_start(&self, line: LineIndex) -> ByteOffset {
        ByteOffset(self.rope.line_to_byte(line.0.min(self.line_count() - 1)))
    }

    #[must_use]
    pub fn line_end(&self, line: LineIndex) -> ByteOffset {
        ByteOffset(self.line_start(line).0 + self.line_len(line))
    }

    /// Clamps an offset into the buffer and down to a character boundary.
    #[must_use]
    pub fn clamp(&self, offset: ByteOffset) -> ByteOffset {
        let clamped = offset.0.min(self.rope.len_bytes());
        ByteOffset(self.rope.char_to_byte(self.rope.byte_to_char(clamped)))
    }

    /// Converts an offset to a line and byte column. Out-of-range offsets are clamped.
    #[must_use]
    pub fn point_at(&self, offset: ByteOffset) -> Point {
        let offset = self.clamp(offset);
        let line = self.rope.byte_to_line(offset.0);
        Point::new(line, offset.0 - self.rope.line_to_byte(line))
    }

    /// Converts a point to an offset, clamping the line into the buffer and the column
    /// into the line.
    #[must_use]
    pub fn offset_at(&self, point: Point) -> ByteOffset {
        let line = point.line.min(self.last_line());
        let column = point.column.min(self.line_len(line));
        self.clamp(ByteOffset(self.line_start(line).0 + column))
    }

    /// The number of characters before `point.column` in its line: the caret column shown
    /// in the status bar, zero-based.
    #[must_use]
    pub fn char_column(&self, point: Point) -> usize {
        let line = self.line_slice(point.line.min(self.last_line()));
        line.byte_to_char(point.column.min(line.len_bytes()))
    }

    #[must_use]
    pub fn prev_char(&self, offset: ByteOffset) -> ByteOffset {
        let index = self.rope.byte_to_char(self.clamp(offset).0);
        ByteOffset(self.rope.char_to_byte(index.saturating_sub(1)))
    }

    #[must_use]
    pub fn next_char(&self, offset: ByteOffset) -> ByteOffset {
        let index = self.rope.byte_to_char(self.clamp(offset).0);
        ByteOffset(
            self.rope
                .char_to_byte((index + 1).min(self.rope.len_chars())),
        )
    }

    fn check_range(&self, range: &ByteRange) -> Result<(), TextError> {
        let (start, end, len) = (range.start.0, range.end.0, self.rope.len_bytes());
        if start > end || end > len {
            return Err(TextError::RangeOutOfBounds { start, end, len });
        }
        for offset in [start, end] {
            if self.rope.char_to_byte(self.rope.byte_to_char(offset)) != offset {
                return Err(TextError::NotCharBoundary(offset));
            }
        }
        Ok(())
    }

    pub fn text(&self, range: ByteRange) -> Result<String, TextError> {
        self.check_range(&range)?;
        Ok(self.rope.byte_slice(range.start.0..range.end.0).to_string())
    }

    /// The chunk of text containing `offset`, starting at `offset`. Empty at the end of the
    /// buffer. This is the read callback incremental parsers want.
    #[must_use]
    pub fn chunk_at(&self, offset: ByteOffset) -> &str {
        if offset.0 >= self.rope.len_bytes() {
            return "";
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(offset.0);
        chunk.get(offset.0 - chunk_start..).unwrap_or("")
    }

    pub fn replace(&mut self, range: ByteRange, text: &str) -> Result<Edit, TextError> {
        self.check_range(&range)?;
        let start_point = self.point_at(range.start);
        let old_end_point = self.point_at(range.end);
        let start_char = self.rope.byte_to_char(range.start.0);
        let end_char = self.rope.byte_to_char(range.end.0);
        self.rope.remove(start_char..end_char);
        self.rope.insert(start_char, text);
        self.version += 1;
        let new_end = ByteOffset(range.start.0 + text.len());
        Ok(Edit {
            start: range.start,
            old_end: range.end,
            new_end,
            start_point,
            old_end_point,
            new_end_point: self.point_at(new_end),
        })
    }

    /// The range of the word (letters, digits and `_`) touching `offset`, or `None` if
    /// there is no word character on either side of it.
    #[must_use]
    pub fn word_range_at(&self, offset: ByteOffset) -> Option<ByteRange> {
        let caret = self.rope.byte_to_char(self.clamp(offset).0);
        let mut start = caret;
        while start > 0 && is_word_char(self.rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = caret;
        while end < self.rope.len_chars() && is_word_char(self.rope.char(end)) {
            end += 1;
        }
        (start < end).then(|| {
            ByteOffset(self.rope.char_to_byte(start))..ByteOffset(self.rope.char_to_byte(end))
        })
    }
}

impl std::fmt::Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.rope.chunks().try_for_each(|chunk| f.write_str(chunk))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn range(start: usize, end: usize) -> ByteRange {
        ByteOffset(start)..ByteOffset(end)
    }

    #[test]
    fn lines_exclude_their_breaks() {
        let buffer = Buffer::new("one\r\ntwo\n\nlast");
        assert_eq!(buffer.line_count(), 4);
        let lines: Vec<_> = (0..4)
            .map(|i| buffer.line(LineIndex(i)).into_owned())
            .collect();
        assert_eq!(lines, ["one", "two", "", "last"]);
        assert_eq!(buffer.line_start(LineIndex(1)), ByteOffset(5));
        assert_eq!(buffer.line_end(LineIndex(1)), ByteOffset(8));
        assert_eq!(
            buffer.line(LineIndex(99)),
            "last",
            "clamps to the last line"
        );
    }

    #[test]
    fn empty_and_trailing_newline() {
        let empty = Buffer::new("");
        assert!(empty.is_empty());
        assert_eq!(empty.line_count(), 1);
        assert_eq!(empty.line(LineIndex(0)), "");
        let trailing = Buffer::new("a\n");
        assert_eq!(trailing.line_count(), 2);
        assert_eq!(trailing.line(LineIndex(1)), "");
    }

    #[test]
    fn points_and_offsets_round_trip_and_clamp() {
        let buffer = Buffer::new("héllo\nwörld");
        assert_eq!(buffer.point_at(ByteOffset(8)), Point::new(1, 1));
        assert_eq!(buffer.offset_at(Point::new(1, 1)), ByteOffset(8));
        assert_eq!(
            buffer.offset_at(Point::new(0, 99)),
            ByteOffset(6),
            "column clamps to line end"
        );
        assert_eq!(
            buffer.offset_at(Point::new(9, 3)),
            ByteOffset(10),
            "line clamps to last"
        );
        assert_eq!(buffer.point_at(ByteOffset(999)), Point::new(1, 6));
        assert_eq!(
            buffer.clamp(ByteOffset(2)),
            ByteOffset(1),
            "inside é snaps back"
        );
        assert_eq!(buffer.char_column(Point::new(0, 3)), 2);
    }

    #[test]
    fn char_stepping_respects_multibyte_characters() {
        let buffer = Buffer::new("aé𝄞");
        assert_eq!(buffer.next_char(ByteOffset(0)), ByteOffset(1));
        assert_eq!(buffer.next_char(ByteOffset(1)), ByteOffset(3));
        assert_eq!(buffer.next_char(ByteOffset(3)), ByteOffset(7));
        assert_eq!(buffer.next_char(ByteOffset(7)), ByteOffset(7));
        assert_eq!(buffer.prev_char(ByteOffset(7)), ByteOffset(3));
        assert_eq!(buffer.prev_char(ByteOffset(0)), ByteOffset(0));
    }

    #[test]
    fn replace_reports_the_edit() {
        let mut buffer = Buffer::new("fn main() {}\n");
        let edit = buffer.replace(range(3, 7), "start\nrun").unwrap();
        assert_eq!(buffer.to_string(), "fn start\nrun() {}\n");
        assert_eq!(edit.start_point, Point::new(0, 3));
        assert_eq!(edit.old_end_point, Point::new(0, 7));
        assert_eq!(edit.new_end, ByteOffset(12));
        assert_eq!(edit.new_end_point, Point::new(1, 3));
        assert_eq!(buffer.version(), 1);
    }

    #[test]
    fn rejects_bad_ranges() {
        let mut buffer = Buffer::new("é");
        assert_eq!(
            buffer.replace(range(0, 9), ""),
            Err(TextError::RangeOutOfBounds {
                start: 0,
                end: 9,
                len: 2
            })
        );
        assert!(matches!(
            buffer.text(range(2, 1)),
            Err(TextError::RangeOutOfBounds { .. })
        ));
        assert_eq!(buffer.text(range(1, 2)), Err(TextError::NotCharBoundary(1)));
        assert_eq!(
            buffer.version(),
            0,
            "failed edits leave the buffer untouched"
        );
    }

    #[test]
    fn word_ranges() {
        let buffer = Buffer::new("let foo_bar = naïve2;");
        assert_eq!(buffer.word_range_at(ByteOffset(6)), Some(range(4, 11)));
        assert_eq!(
            buffer.word_range_at(ByteOffset(4)),
            Some(range(4, 11)),
            "caret at start"
        );
        assert_eq!(
            buffer.word_range_at(ByteOffset(11)),
            Some(range(4, 11)),
            "caret at end"
        );
        assert_eq!(buffer.word_range_at(ByteOffset(16)), Some(range(14, 21)));
        assert_eq!(
            buffer.word_range_at(ByteOffset(12)),
            None,
            "between space and ="
        );
        assert_eq!(Buffer::new("").word_range_at(ByteOffset(0)), None);
    }

    #[test]
    fn chunks_cover_the_text() {
        let text = "line of text\n".repeat(2000);
        let buffer = Buffer::new(&text);
        let mut rebuilt = String::new();
        let mut offset = ByteOffset(0);
        loop {
            let chunk = buffer.chunk_at(offset);
            if chunk.is_empty() {
                break;
            }
            rebuilt.push_str(chunk);
            offset = ByteOffset(offset.0 + chunk.len());
        }
        assert_eq!(rebuilt, text);
        assert_eq!(buffer.chunk_at(ByteOffset(5)).as_bytes()[0], b'o');
    }

    fn boundary(text: &str, index: usize) -> usize {
        let mut index = index.min(text.len());
        while !text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    proptest! {
        #[test]
        fn replace_matches_string_model(
            initial in "(?s)\\PC{0,40}|[a\\n\\r é𝄞]{0,40}",
            edits in prop::collection::vec((0usize..64, 0usize..64, "[a\\n\\r é𝄞]{0,6}"), 0..12),
        ) {
            let mut model = initial.clone();
            let mut buffer = Buffer::new(&initial);
            for (a, b, text) in edits {
                let (a, b) = (boundary(&model, a), boundary(&model, b));
                let (start, end) = (a.min(b), a.max(b));
                let edit = buffer.replace(range(start, end), &text).unwrap();
                model.replace_range(start..end, &text);
                prop_assert_eq!(buffer.to_string(), model.clone());
                prop_assert_eq!(edit.new_end.0, start + text.len());
                // Equal except when the edit ends between a \r and its \n.
                prop_assert!(buffer.offset_at(edit.new_end_point) <= edit.new_end);
            }
            prop_assert_eq!(buffer.line_count(), model.matches('\n').count() + 1);
        }

        #[test]
        fn offsets_round_trip_through_points(text in "[a\\n\\r é𝄞]{0,60}") {
            let buffer = Buffer::new(&text);
            for (index, _) in text.char_indices().chain([(text.len(), ' ')]) {
                let offset = ByteOffset(index);
                let point = buffer.point_at(offset);
                // A caret between \r and \n is not addressable as a point: it clamps to
                // the line end before the \r.
                let in_crlf = text[..index].ends_with('\r') && text[index..].starts_with('\n');
                if !in_crlf {
                    prop_assert_eq!(buffer.offset_at(point), offset);
                }
                prop_assert!(buffer.offset_at(point) <= offset);
            }
        }
    }
}
