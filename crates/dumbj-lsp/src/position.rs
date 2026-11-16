use dumbj_text::{Buffer, ByteOffset, LineIndex, Point};
use lsp_types::{Position, Range};

#[must_use]
pub fn to_lsp(buffer: &Buffer, offset: ByteOffset) -> Position {
    let point = buffer.point_at(offset);
    let line = buffer.line(point.line);
    let end = point.column.min(line.len());
    let column = line
        .get(..end)
        .map_or(0, |prefix| prefix.encode_utf16().count());
    Position::new(clamp_u32(point.line.get()), clamp_u32(column))
}

/// A column past the line end clamps to the line end, a line past the end to the buffer end,
/// and a column inside a surrogate pair rounds down.
#[must_use]
pub fn from_lsp(buffer: &Buffer, position: Position) -> ByteOffset {
    let line = position.line as usize;
    if line >= buffer.line_count() {
        return buffer.len();
    }
    let text = buffer.line(LineIndex(line));
    let target = position.character as usize;
    let mut units = 0;
    let mut bytes = text.len();
    for (index, ch) in text.char_indices() {
        let next = units + ch.len_utf16();
        if next > target {
            bytes = index;
            break;
        }
        units = next;
    }
    buffer.offset_at(Point::new(line, bytes))
}

#[must_use]
pub fn to_lsp_range(buffer: &Buffer, range: std::ops::Range<ByteOffset>) -> Range {
    Range::new(to_lsp(buffer, range.start), to_lsp(buffer, range.end))
}

#[must_use]
pub fn from_lsp_range(buffer: &Buffer, range: Range) -> std::ops::Range<ByteOffset> {
    from_lsp(buffer, range.start)..from_lsp(buffer, range.end)
}

fn clamp_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn counts_utf16_units() {
        let buffer = Buffer::new("a\u{1f600}b\nxy");
        assert_eq!(to_lsp(&buffer, ByteOffset(5)), Position::new(0, 3));
        assert_eq!(from_lsp(&buffer, Position::new(0, 3)), ByteOffset(5));
        assert_eq!(from_lsp(&buffer, Position::new(0, 2)), ByteOffset(1));
    }

    #[test]
    fn clamps_out_of_range() {
        let buffer = Buffer::new("ab\r\ncd");
        assert_eq!(from_lsp(&buffer, Position::new(0, 99)), ByteOffset(2));
        assert_eq!(from_lsp(&buffer, Position::new(9, 0)), buffer.len());
        assert_eq!(from_lsp(&buffer, Position::new(1, 99)), buffer.len());
    }

    proptest! {
        #[test]
        fn offsets_round_trip(text in "[a-z\u{e9}\u{1f600}\u{65e5} \n]{0,60}|([a-z\u{1f600}]{0,8}\r\n){0,6}[a-z]{0,3}", seed in any::<usize>()) {
            let buffer = Buffer::new(&text);
            let boundaries: Vec<usize> = text
                .char_indices()
                .map(|(i, _)| i)
                .chain([text.len()])
                .filter(|&i| !(text.as_bytes().get(i) == Some(&b'\n') && i > 0 && text.as_bytes()[i - 1] == b'\r'))
                .collect();
            let offset = ByteOffset(boundaries[seed % boundaries.len()]);
            let position = to_lsp(&buffer, offset);
            prop_assert_eq!(from_lsp(&buffer, position), offset);
        }

        #[test]
        fn any_position_gives_a_valid_offset(text in "\\PC{0,40}", line in 0u32..6, character in 0u32..60) {
            let buffer = Buffer::new(&text);
            let offset = from_lsp(&buffer, Position::new(line, character));
            prop_assert!(offset <= buffer.len());
            prop_assert_eq!(buffer.clamp(offset), offset);
        }
    }
}
