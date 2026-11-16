use std::ops::Range;

use dumbj_settings::UiRole;
use dumbj_text::{Buffer, ByteRange};
use gpui::Context;

use super::{EditorView, LoadState};

/// How one side of a diff differs from the other, drawn behind the text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DiffMarks {
    /// Changed line ranges and their colour, sorted.
    pub(crate) lines: Vec<(Range<usize>, UiRole)>,
    /// The changed words inside modified lines, sorted.
    pub(crate) words: Vec<ByteRange>,
    /// Lines above which the other side has lines this side lacks.
    pub(crate) gaps: Vec<usize>,
}

impl DiffMarks {
    pub(super) fn role_of(&self, line: usize) -> Option<UiRole> {
        self.lines
            .iter()
            .find(|(lines, _)| lines.contains(&line))
            .map(|(_, role)| *role)
    }

    pub(super) fn words_in(&self, line: &ByteRange) -> &[ByteRange] {
        let from = self.words.partition_point(|word| word.end <= line.start);
        let to = self.words.partition_point(|word| word.start < line.end);
        self.words.get(from..to.max(from)).unwrap_or(&[])
    }
}

impl EditorView {
    pub(crate) fn set_diff_marks(&mut self, marks: DiffMarks, cx: &mut Context<Self>) {
        if self.diff_marks != marks {
            self.diff_marks = marks;
            cx.notify();
        }
    }

    /// A cheap copy of the text to compare, once loading has finished. A file that cannot
    /// be read (deleted from the working tree) compares as empty.
    pub(crate) fn diff_snapshot(&self) -> Option<Buffer> {
        match self.state {
            LoadState::Loading => None,
            LoadState::Failed(_) => Some(Buffer::new("")),
            LoadState::Ready => Some(self.model.buffer().clone()),
        }
    }

    /// The buffer line at the top of the view; fractional while it is partly scrolled off.
    pub(crate) fn top_line(&self) -> f64 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "scroll_top is a small non-negative row number"
        )]
        let line = self.model.folds.line_of(self.scroll_top.floor() as usize) as f64;
        line + self.scroll_top.fract()
    }

    pub(crate) fn set_top_line(&mut self, line: f64, cx: &mut Context<Self>) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "a line number"
        )]
        let row = self.model.folds.row_of(line.max(0.0).floor() as usize) as f64;
        let top = (row + line.max(0.0).fract()).clamp(0.0, self.max_scroll_top());
        if (top - self.scroll_top).abs() > f64::EPSILON {
            self.scroll_top = top;
            self.reveal = None;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use dumbj_text::ByteOffset;

    use super::*;

    #[test]
    fn words_are_found_by_line() {
        let range = |start: usize, end: usize| ByteOffset(start)..ByteOffset(end);
        let marks = DiffMarks {
            lines: vec![(2..4, UiRole::Info)],
            words: vec![range(3, 5), range(12, 14), range(30, 31)],
            gaps: Vec::new(),
        };
        assert_eq!(marks.words_in(&range(10, 20)), [range(12, 14)]);
        assert_eq!(marks.words_in(&range(0, 13)).len(), 2);
        assert_eq!(marks.words_in(&range(15, 29)), []);
        assert_eq!(marks.role_of(3), Some(UiRole::Info));
        assert_eq!(marks.role_of(4), None);
    }
}
