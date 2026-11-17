use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

/// Line numbers are zero-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub kind: HunkKind,
    /// The lines of the base text that were replaced or removed; empty for `Added`.
    pub old: Range<usize>,
    /// The lines of the current text that replace them; empty for `Deleted`, where
    /// `new.start` is the line the removed lines would come before.
    pub new: Range<usize>,
}

/// Lines are split on `\n` and keep their terminator, so a trailing `\r` is part of the line
/// (a CRLF/LF change shows as modified) and `"a\nb"` against `"a\nb\n"` is one `Modified`
/// hunk covering the last line.
pub fn line_hunks(base: &str, current: &str) -> Vec<Hunk> {
    line_hunks_with(base, current, DiffOptions::default())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffOptions {
    /// Lines that differ only in whitespace (leading, trailing, runs inside the line, and CR
    /// at the end) count as equal.
    pub ignore_whitespace: bool,
}

/// With `ignore_whitespace` a missing final newline does not count as a change either.
pub fn line_hunks_with(base: &str, current: &str, options: DiffOptions) -> Vec<Hunk> {
    if base == current {
        return Vec::new();
    }
    if options.ignore_whitespace {
        return compute_hunks(&squeeze(base), &squeeze(current));
    }
    compute_hunks(base, current)
}

fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut words = line.split_whitespace();
        if let Some(first) = words.next() {
            out.push_str(first);
            for word in words {
                out.push(' ');
                out.push_str(word);
            }
        }
        out.push('\n');
    }
    out
}

fn compute_hunks(base: &str, current: &str) -> Vec<Hunk> {
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

pub fn old_text(base: &str, hunk: &Hunk) -> String {
    base.split_inclusive('\n')
        .skip(hunk.old.start)
        .take(hunk.old.len())
        .collect()
}

/// The parts of two lines that differ, as byte ranges into each (sorted, non-overlapping, on
/// char boundaries), computed over words: runs of alphanumerics and underscores are one
/// token, whitespace runs are one token and every other char is its own token. A trailing
/// line terminator is ignored. Identical lines give two empty lists. When the lines share
/// less than a third of their tokens (whitespace tokens do not count) the whole of each line is returned as one range
/// (highlighting scattered coincidences is noise); an empty line has no range.
pub fn word_changes(old_line: &str, new_line: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let old_line = strip_terminator(old_line);
    let new_line = strip_terminator(new_line);
    if old_line == new_line {
        return (Vec::new(), Vec::new());
    }
    let old_tokens = tokens(old_line);
    let new_tokens = tokens(new_line);
    let mut input = InternedInput::default();
    input.update_before(old_tokens.iter().map(|t| &old_line[t.clone()]));
    input.update_after(new_tokens.iter().map(|t| &new_line[t.clone()]));
    let diff = Diff::compute(Algorithm::Histogram, &input);

    let mut old_changes = Vec::new();
    let mut new_changes = Vec::new();
    let mut shared = significant(old_line, &old_tokens);
    for hunk in diff.hunks() {
        let removed = old_tokens.get(hunk.before.start as usize..hunk.before.end as usize);
        shared -= removed.map_or(0, |t| significant(old_line, t));
        if let Some(range) = span(&old_tokens, hunk.before.start..hunk.before.end) {
            old_changes.push(range);
        }
        if let Some(range) = span(&new_tokens, hunk.after.start..hunk.after.end) {
            new_changes.push(range);
        }
    }
    let total = significant(old_line, &old_tokens).max(significant(new_line, &new_tokens));
    if shared * 3 < total {
        let whole = |line: &str| -> Vec<Range<usize>> {
            (!line.is_empty())
                .then_some(0..line.len())
                .into_iter()
                .collect()
        };
        return (whole(old_line), whole(new_line));
    }
    (old_changes, new_changes)
}

fn strip_terminator(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn significant(line: &str, tokens: &[Range<usize>]) -> usize {
    tokens
        .iter()
        .filter(|t| !line[(*t).clone()].starts_with(char::is_whitespace))
        .count()
}

fn span(tokens: &[Range<usize>], hunk: Range<u32>) -> Option<Range<usize>> {
    if hunk.start >= hunk.end {
        return None;
    }
    let first = tokens.get(hunk.start as usize)?;
    let last = tokens.get(hunk.end as usize - 1)?;
    Some(first.start..last.end)
}

fn tokens(line: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut previous = Class::Other;
    for (at, c) in line.char_indices() {
        let current = class(c);
        match out.last_mut() {
            Some(last) if current == previous && current != Class::Other => {
                last.end = at + c.len_utf8();
            }
            _ => out.push(at..at + c.len_utf8()),
        }
        previous = current;
    }
    out
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

    fn ws() -> DiffOptions {
        DiffOptions {
            ignore_whitespace: true,
        }
    }

    #[test]
    fn whitespace_only_differences() {
        let base = "a b\n  c\nd\r\ne\n";
        let current = "a   b\nc  \nd\ne";
        assert_eq!(line_hunks_with(base, current, ws()), []);
        assert_eq!(
            line_hunks_with(base, current, DiffOptions::default()).len(),
            1
        );
        assert_eq!(
            line_hunks_with("a\nb\n", "a\n\n  \nb\n", ws()),
            [h(HunkKind::Added, 1..1, 1..3)]
        );
        assert_eq!(
            line_hunks_with("ab\nc\n", "a b\nc\n", ws()),
            [h(HunkKind::Modified, 0..1, 0..1)]
        );
        assert_eq!(
            line_hunks_with("a\nb\nc\n", "a\n  X\nc\n", ws()),
            [h(HunkKind::Modified, 1..2, 1..2)]
        );
        assert_eq!(line_hunks_with("", "", ws()), []);
    }

    fn texts<'a>(line: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|r| &line[r.clone()]).collect()
    }

    #[test]
    fn word_changes_for_a_renamed_identifier() {
        let (old, new) = ("let total = count + 1;", "let sum = count + 1;");
        let (o, n) = word_changes(old, new);
        assert_eq!(texts(old, &o), ["total"]);
        assert_eq!(texts(new, &n), ["sum"]);
    }

    #[test]
    fn word_changes_for_an_inserted_argument() {
        let (old, new) = ("call(a, b)", "call(a, x, b)");
        let (o, n) = word_changes(old, new);
        assert_eq!(texts(old, &o), Vec::<&str>::new());
        assert_eq!(texts(new, &n), ["x, "]);
    }

    #[test]
    fn word_changes_ignore_line_terminators_and_identical_lines() {
        assert_eq!(word_changes("same\r\n", "same\n"), (vec![], vec![]));
        assert_eq!(word_changes("", ""), (vec![], vec![]));
    }

    #[test]
    fn word_changes_for_totally_different_lines_cover_each_line() {
        let (o, n) = word_changes("alpha beta gamma", "x = 1;");
        assert_eq!(o, one(0, 16));
        assert_eq!(n, one(0, 6));
    }

    fn one(start: usize, end: usize) -> Vec<Range<usize>> {
        vec![Range { start, end }]
    }

    #[test]
    fn word_changes_of_an_empty_line() {
        assert_eq!(word_changes("", "x"), (vec![], one(0, 1)));
        assert_eq!(word_changes("x", ""), (one(0, 1), vec![]));
    }

    #[test]
    fn word_changes_with_unicode() {
        let (old, new) = ("naïve café = 1;", "naïve thé = 1;");
        let (o, n) = word_changes(old, new);
        assert_eq!(texts(old, &o), ["café"]);
        assert_eq!(texts(new, &n), ["thé"]);
        assert_eq!(o, one(7, 12));
    }

    proptest! {
        #[test]
        fn word_changes_are_sorted_in_bounds_and_on_char_boundaries(
            old in "[a-c é_ ;=]{0,16}",
            new in "[a-c é_ ;=]{0,16}",
        ) {
            let (o, n) = word_changes(&old, &new);
            for (line, ranges) in [(&old, &o), (&new, &n)] {
                let mut end = 0;
                for r in ranges {
                    prop_assert!(r.start >= end && r.start < r.end && r.end <= line.len());
                    prop_assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
                    end = r.end;
                }
            }
            prop_assert_eq!(old == new, o.is_empty() && n.is_empty());
        }

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
