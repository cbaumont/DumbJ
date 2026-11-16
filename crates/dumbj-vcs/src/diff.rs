use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

/// A run of changed lines. Line numbers are zero-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    /// The lines of the base text that were replaced or removed; empty for `Added`.
    pub old: Range<usize>,
    /// The lines of the current text that replace them; empty for `Deleted`, where
    /// `new.start` is the line the removed lines would come before.
    pub new: Range<usize>,
}

/// The hunks that turn `base` into `current`, in order.
///
/// Lines are split on `\n` and keep their terminator, so a trailing `\r` is part of the line
/// (a CRLF/LF change shows as modified) and `"a\nb"` against `"a\nb\n"` is one `Modified`
/// hunk covering the last line.
pub fn line_hunks(base: &str, current: &str) -> Vec<Hunk> {
    if base == current {
        return Vec::new();
    }
    let input = InternedInput::new(base, current);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    diff.hunks()
        .map(|h| {
            let old = h.before.start as usize..h.before.end as usize;
            let new = h.after.start as usize..h.after.end as usize;
            let kind = match (old.is_empty(), new.is_empty()) {
                (true, _) => HunkKind::Added,
                (_, true) => HunkKind::Deleted,
                _ => HunkKind::Modified,
            };
            Hunk { kind, old, new }
        })
        .collect()
}

/// The text of `base`'s lines `hunk.old`, each with its line terminator, for putting them
/// back. Empty for `Added`.
pub fn old_text(base: &str, hunk: &Hunk) -> String {
    base.split_inclusive('\n')
        .skip(hunk.old.start)
        .take(hunk.old.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn h(kind: HunkKind, old: Range<usize>, new: Range<usize>) -> Hunk {
        Hunk { kind, old, new }
    }

    #[test]
    fn identical_texts_have_no_hunks() {
        assert_eq!(line_hunks("a\nb\n", "a\nb\n"), Vec::<Hunk>::new());
        assert_eq!(line_hunks("", ""), Vec::<Hunk>::new());
    }

    #[test]
    fn added_at_start_middle_and_end() {
        assert_eq!(
            line_hunks("b\nc\n", "a\nb\nc\n"),
            [h(HunkKind::Added, 0..0, 0..1)]
        );
        assert_eq!(
            line_hunks("a\nc\n", "a\nb\nc\n"),
            [h(HunkKind::Added, 1..1, 1..2)]
        );
        assert_eq!(
            line_hunks("a\nb\n", "a\nb\nc\n"),
            [h(HunkKind::Added, 2..2, 2..3)]
        );
    }

    #[test]
    fn deleted_lines() {
        assert_eq!(
            line_hunks("a\nb\nc\n", "a\nc\n"),
            [h(HunkKind::Deleted, 1..2, 1..1)]
        );
        assert_eq!(
            line_hunks("a\nb\n", "a\n"),
            [h(HunkKind::Deleted, 1..2, 1..1)]
        );
    }

    #[test]
    fn modified_lines() {
        assert_eq!(
            line_hunks("a\nb\nc\n", "a\nX\nc\n"),
            [h(HunkKind::Modified, 1..2, 1..2)]
        );
        assert_eq!(
            line_hunks("a\nb\nc\n", "a\nX\nY\nZ\n"),
            [h(HunkKind::Modified, 1..3, 1..4)]
        );
    }

    #[test]
    fn mixed_hunks_are_in_order() {
        let hunks = line_hunks("a\nb\nc\nd\ne\n", "z\na\nc\nD\ne\nf\n");
        assert_eq!(
            hunks,
            [
                h(HunkKind::Added, 0..0, 0..1),
                h(HunkKind::Deleted, 1..2, 2..2),
                h(HunkKind::Modified, 3..4, 3..4),
                h(HunkKind::Added, 5..5, 5..6),
            ]
        );
    }

    #[test]
    fn crlf_against_lf_is_modified() {
        assert_eq!(
            line_hunks("a\nb\n", "a\r\nb\n"),
            [h(HunkKind::Modified, 0..1, 0..1)]
        );
    }

    #[test]
    fn empty_base_and_empty_current() {
        assert_eq!(line_hunks("", "a\nb\n"), [h(HunkKind::Added, 0..0, 0..2)]);
        assert_eq!(line_hunks("a\nb\n", ""), [h(HunkKind::Deleted, 0..2, 0..0)]);
    }

    #[test]
    fn missing_final_newline_modifies_the_last_line() {
        assert_eq!(
            line_hunks("a\nb", "a\nb\n"),
            [h(HunkKind::Modified, 1..2, 1..2)]
        );
    }

    #[test]
    fn old_text_returns_the_replaced_lines() {
        let base = "a\nb\r\nc\nd";
        let hunk = h(HunkKind::Modified, 1..3, 1..2);
        assert_eq!(old_text(base, &hunk), "b\r\nc\n");
        let last = h(HunkKind::Deleted, 3..4, 3..3);
        assert_eq!(old_text(base, &last), "d");
        assert_eq!(old_text(base, &h(HunkKind::Added, 1..1, 1..2)), "");
    }

    fn apply(base: &str, current: &str, hunks: &[Hunk]) -> String {
        let old: Vec<&str> = base.split_inclusive('\n').collect();
        let new: Vec<&str> = current.split_inclusive('\n').collect();
        let mut out = String::new();
        let mut at = 0;
        for hunk in hunks {
            out.extend(old[at..hunk.old.start].iter().copied());
            out.extend(new[hunk.new.clone()].iter().copied());
            at = hunk.old.end;
        }
        out.extend(old[at..].iter().copied());
        out
    }

    proptest! {
        #[test]
        fn hunks_reproduce_current(
            base in "([abc]{0,2}(\n|\r\n)?){0,12}",
            current in "([abc]{0,2}(\n|\r\n)?){0,12}",
        ) {
            let hunks = line_hunks(&base, &current);
            prop_assert_eq!(apply(&base, &current, &hunks), current);
        }
    }
}
