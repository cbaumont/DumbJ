use crate::buffer::Buffer;
use crate::position::{ByteOffset, ByteRange, LineIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// When `false`, ASCII letters match regardless of case.
    pub case_sensitive: bool,
}

/// Finds every non-overlapping occurrence of `needle`, in order.
///
/// Matches never span lines, so a needle containing a line break finds nothing. `cancelled`
/// is polled once per line; when it returns `true` the matches found so far are returned.
pub fn find_all(
    buffer: &Buffer,
    needle: &str,
    options: SearchOptions,
    cancelled: impl Fn() -> bool,
) -> Vec<ByteRange> {
    let mut matches = Vec::new();
    if needle.is_empty() {
        return matches;
    }
    let folded_needle = needle.to_ascii_lowercase();
    for index in 0..buffer.line_count() {
        if cancelled() {
            break;
        }
        let line_index = LineIndex(index);
        let line = buffer.line(line_index);
        let line_start = buffer.line_start(line_index).get();
        let mut push_matches = |haystack: &str, needle: &str| {
            // ASCII case folding keeps byte offsets identical to the original line.
            for (at, found) in haystack.match_indices(needle) {
                let start = line_start + at;
                matches.push(ByteOffset(start)..ByteOffset(start + found.len()));
            }
        };
        if options.case_sensitive {
            push_matches(&line, needle);
        } else {
            push_matches(&line.to_ascii_lowercase(), &folded_needle);
        }
    }
    matches
}

/// The index of the first match starting at or after `offset`, wrapping to the first match.
#[must_use]
pub fn next_match(matches: &[ByteRange], offset: ByteOffset) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let index = matches.partition_point(|found| found.start < offset);
    Some(if index == matches.len() { 0 } else { index })
}

/// The index of the last match starting before `offset`, wrapping to the last match.
#[must_use]
pub fn previous_match(matches: &[ByteRange], offset: ByteOffset) -> Option<usize> {
    let index = matches.partition_point(|found| found.start < offset);
    index.checked_sub(1).or(matches.len().checked_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn starts(buffer: &Buffer, needle: &str, case_sensitive: bool) -> Vec<usize> {
        find_all(buffer, needle, SearchOptions { case_sensitive }, || false)
            .into_iter()
            .map(|found| found.start.get())
            .collect()
    }

    #[test]
    fn finds_matches_across_lines_in_order() {
        let buffer = Buffer::new("foo Foo\nbar foo\r\nfoofoo");
        assert_eq!(starts(&buffer, "foo", true), [0, 12, 17, 20]);
        assert_eq!(starts(&buffer, "foo", false), [0, 4, 12, 17, 20]);
        assert_eq!(starts(&buffer, "FOO", false), [0, 4, 12, 17, 20]);
        assert_eq!(starts(&buffer, "", true), [] as [usize; 0]);
        assert_eq!(
            starts(&buffer, "o\nb", true),
            [] as [usize; 0],
            "never spans lines"
        );
    }

    #[test]
    fn overlapping_candidates_do_not_overlap() {
        assert_eq!(starts(&Buffer::new("aaaa"), "aa", true), [0, 2]);
    }

    #[test]
    fn offsets_stay_correct_after_multibyte_text() {
        let buffer = Buffer::new("é ÉX x");
        assert_eq!(starts(&buffer, "x", false), [5, 7]);
        assert_eq!(
            starts(&buffer, "é", false),
            [0],
            "only ASCII is case folded"
        );
    }

    #[test]
    fn cancellation_stops_early() {
        let buffer = Buffer::new("a\na\na");
        let found = find_all(&buffer, "a", SearchOptions::default(), || true);
        assert_eq!(found, []);
    }

    #[test]
    fn next_and_previous_wrap() {
        let buffer = Buffer::new("a.a.a");
        let matches = find_all(&buffer, "a", SearchOptions::default(), || false);
        assert_eq!(next_match(&matches, ByteOffset(0)), Some(0));
        assert_eq!(next_match(&matches, ByteOffset(1)), Some(1));
        assert_eq!(
            next_match(&matches, ByteOffset(5)),
            Some(0),
            "wraps to the top"
        );
        assert_eq!(previous_match(&matches, ByteOffset(4)), Some(1));
        assert_eq!(
            previous_match(&matches, ByteOffset(0)),
            Some(2),
            "wraps to the bottom"
        );
        assert_eq!(next_match(&[], ByteOffset(0)), None);
        assert_eq!(previous_match(&[], ByteOffset(0)), None);
    }
}
