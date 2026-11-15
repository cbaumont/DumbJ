use dumbj_text::{Buffer, ByteOffset, ByteRange, LineIndex, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// To the first non-blank character of the line, or to column 0 if already there.
    LineStart,
    LineEnd,
    TextStart,
    TextEnd,
    PageUp(usize),
    PageDown(usize),
    WordLeft,
    WordRight,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Default)]
pub(crate) struct EditorModel {
    buffer: Buffer,
    anchor: ByteOffset,
    /// The moving end of the selection, where the caret is drawn.
    caret: ByteOffset,
    /// The character column vertical movement tries to return to.
    goal_column: Option<usize>,
    /// Each Extend Selection step as (before, after), for Shrink Selection.
    extended: Vec<(ByteRange, ByteRange)>,
    pub(crate) folds: Folds,
}

impl EditorModel {
    pub(crate) fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            ..Self::default()
        }
    }

    pub(crate) fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub(crate) fn caret(&self) -> ByteOffset {
        self.caret
    }

    pub(crate) fn caret_point(&self) -> Point {
        self.buffer.point_at(self.caret)
    }

    pub(crate) fn selection(&self) -> ByteRange {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    pub(crate) fn selected_text(&self) -> String {
        self.buffer.text(self.selection()).unwrap_or_default()
    }

    pub(crate) fn move_to(&mut self, offset: ByteOffset, extend: bool) {
        self.caret = self.buffer.clamp(offset);
        self.folds
            .reveal(self.buffer.point_at(self.caret).line.get());
        if !extend {
            self.anchor = self.caret;
        }
        self.goal_column = None;
    }

    pub(crate) fn select(&mut self, range: ByteRange) {
        self.anchor = self.buffer.clamp(range.start);
        self.caret = self.buffer.clamp(range.end);
        self.folds
            .reveal(self.buffer.point_at(self.caret).line.get());
        self.goal_column = None;
    }

    pub(crate) fn select_all(&mut self) {
        self.select(ByteOffset::ZERO..self.buffer.len());
    }

    pub(crate) fn select_line(&mut self, line: LineIndex) {
        let next = LineIndex(line.get() + 1);
        let end = if next.get() < self.buffer.line_count() {
            self.buffer.line_start(next)
        } else {
            self.buffer.len()
        };
        self.select(self.buffer.line_start(line)..end);
    }

    pub(crate) fn select_word_at(&mut self, offset: ByteOffset) -> bool {
        let word = self.buffer.word_range_at(offset);
        if let Some(range) = word.clone() {
            self.select(range);
        }
        word.is_some()
    }

    pub(crate) fn extend_selection(
        &mut self,
        enclosing: impl Fn(ByteRange) -> Option<ByteRange>,
    ) -> bool {
        let current = self.selection();
        if self
            .extended
            .last()
            .is_some_and(|(_, after)| *after != current)
        {
            self.extended.clear();
        }
        let word = (!self.has_selection())
            .then(|| self.buffer.word_range_at(self.caret))
            .flatten();
        let Some(next) = word.or_else(|| enclosing(current.clone())) else {
            return false;
        };
        let caret = self.caret;
        self.select(next.clone());
        let before = if current.is_empty() {
            caret..caret
        } else {
            current
        };
        self.extended.push((before, next));
        true
    }

    pub(crate) fn shrink_selection(&mut self) -> bool {
        let current = self.selection();
        match self.extended.pop() {
            Some((before, after)) if after == current => {
                if before.is_empty() {
                    self.move_to(before.start, false);
                } else {
                    self.select(before);
                }
                true
            }
            _ => {
                self.extended.clear();
                false
            }
        }
    }

    /// Puts the caret at a one-based line and optional one-based character column.
    pub(crate) fn go_to(&mut self, line: usize, column: Option<usize>) {
        let line = LineIndex(line.saturating_sub(1).min(self.buffer.line_count() - 1));
        let column = column.unwrap_or(1).saturating_sub(1);
        self.move_to(self.offset_at_char_column(line, column), false);
    }

    fn offset_at_char_column(&self, line: LineIndex, column: usize) -> ByteOffset {
        let text = self.buffer.line(line);
        let bytes = text
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(index, _)| index);
        self.buffer.offset_at(Point {
            line,
            column: bytes,
        })
    }

    fn vertical(&mut self, rows: isize) -> ByteOffset {
        let point = self.caret_point();
        let goal = *self
            .goal_column
            .get_or_insert_with(|| self.buffer.char_column(point));
        let last = self.buffer.line_count() - 1;
        let last_row = self.folds.row_count(self.buffer.line_count()) - 1;
        let row = self.folds.row_of(point.line.get());
        let target = self
            .folds
            .line_of(row.saturating_add_signed(rows).min(last_row))
            .min(last);
        let lines = rows;
        if lines < 0 && point.line.get() == 0 {
            return ByteOffset::ZERO;
        }
        if lines > 0 && point.line.get() == last {
            return self.buffer.len();
        }
        self.offset_at_char_column(LineIndex(target), goal)
    }

    fn line_start(&self) -> ByteOffset {
        let point = self.caret_point();
        let text = self.buffer.line(point.line);
        let indent = text.len() - text.trim_start().len();
        let column = if point.column == indent { 0 } else { indent };
        self.buffer.offset_at(Point {
            line: point.line,
            column,
        })
    }

    fn word_left(&self) -> ByteOffset {
        let point = self.caret_point();
        if point.column == 0 {
            return self.buffer.prev_char(self.caret);
        }
        let text = self.buffer.line(point.line);
        let column = text
            .get(..point.column)
            .unwrap_or("")
            .char_indices()
            .rev()
            .skip_while(|(_, c)| !is_word(*c))
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(0, |(index, _)| index);
        self.buffer.offset_at(Point {
            line: point.line,
            column,
        })
    }

    fn word_right(&self) -> ByteOffset {
        let point = self.caret_point();
        let text = self.buffer.line(point.line);
        if point.column >= text.len() {
            return self.buffer.next_char(self.caret);
        }
        let column = text
            .get(point.column..)
            .unwrap_or("")
            .char_indices()
            .skip_while(|(_, c)| !is_word(*c))
            .find(|(_, c)| !is_word(*c))
            .map_or(text.len(), |(index, _)| point.column + index);
        self.buffer.offset_at(Point {
            line: point.line,
            column,
        })
    }

    /// Moves the caret. Without `extend`, Left and Right collapse a selection to its near
    /// edge instead of moving past it.
    pub(crate) fn apply_motion(&mut self, motion: Motion, extend: bool) {
        let selection = self.selection();
        let collapse = !extend && self.has_selection();
        let (target, keep_goal) = match motion {
            Motion::Left if collapse => (selection.start, false),
            Motion::Right if collapse => (selection.end, false),
            Motion::Left => (self.buffer.prev_char(self.caret), false),
            Motion::Right => (self.buffer.next_char(self.caret), false),
            Motion::Up => (self.vertical(-1), true),
            Motion::Down => (self.vertical(1), true),
            Motion::PageUp(lines) => (self.vertical(-lines.cast_signed()), true),
            Motion::PageDown(lines) => (self.vertical(lines.cast_signed()), true),
            Motion::LineStart => (self.line_start(), false),
            Motion::LineEnd => (self.buffer.line_end(self.caret_point().line), false),
            Motion::TextStart => (ByteOffset::ZERO, false),
            Motion::TextEnd => (self.buffer.len(), false),
            Motion::WordLeft => (self.word_left(), false),
            Motion::WordRight => (self.word_right(), false),
        };
        let goal = self.goal_column.filter(|_| keep_goal);
        self.move_to(target, extend);
        self.goal_column = goal;
    }
}

pub(crate) use dumbj_syntax::FoldKind;

/// Lines `start..=end`; folded, `start` stays visible and the rest are hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FoldRegion {
    pub(crate) header: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: FoldKind,
}

impl FoldRegion {
    fn contains(&self, line: usize) -> bool {
        self.header <= line && line <= self.end
    }
}

#[derive(Debug, Default)]
pub(crate) struct Folds {
    /// One region per start line (the largest), sorted by start.
    regions: Vec<FoldRegion>,
    collapsed: std::collections::BTreeSet<usize>,
    /// Hidden line ranges, sorted and merged.
    hidden: Vec<std::ops::Range<usize>>,
}

impl Folds {
    pub(crate) fn set_regions(&mut self, mut regions: Vec<FoldRegion>) {
        regions.retain(|region| region.end > region.start);
        regions.sort_by_key(|region| (region.start, std::cmp::Reverse(region.end)));
        regions.dedup_by_key(|region| region.start);
        self.regions = regions;
        let starts: std::collections::BTreeSet<usize> =
            self.regions.iter().map(|region| region.start).collect();
        self.collapsed.retain(|start| starts.contains(start));
        self.recompute();
    }

    fn recompute(&mut self) {
        let mut hidden: Vec<std::ops::Range<usize>> = self
            .regions
            .iter()
            .filter(|region| self.collapsed.contains(&region.start))
            .map(|region| region.start + 1..region.end + 1)
            .collect();
        hidden.sort_by_key(|range| range.start);
        let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
        for range in hidden {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.hidden = merged;
    }

    pub(crate) fn region_starting(&self, line: usize) -> Option<FoldRegion> {
        self.regions
            .binary_search_by_key(&line, |region| region.start)
            .ok()
            .and_then(|position| self.regions.get(position).copied())
    }

    pub(crate) fn is_collapsed(&self, start: usize) -> bool {
        self.collapsed.contains(&start)
    }

    pub(crate) fn toggle(&mut self, start: usize) -> bool {
        if self.region_starting(start).is_none() {
            return false;
        }
        if !self.collapsed.remove(&start) {
            self.collapsed.insert(start);
        }
        self.recompute();
        true
    }

    /// Collapses the innermost expanded region around `line`.
    pub(crate) fn collapse_at(&mut self, line: usize) -> bool {
        let innermost = self
            .regions
            .iter()
            .filter(|region| region.contains(line) && !self.collapsed.contains(&region.start))
            .min_by_key(|region| region.end - region.start)
            .copied();
        innermost.is_some_and(|region| self.toggle(region.start))
    }

    /// Expands the collapsed region that starts on `line`, else the innermost one around it.
    pub(crate) fn expand_at(&mut self, line: usize) -> bool {
        let target = self
            .regions
            .iter()
            .filter(|region| region.contains(line) && self.collapsed.contains(&region.start))
            .min_by_key(|region| (region.start != line, region.end - region.start))
            .copied();
        target.is_some_and(|region| self.toggle(region.start))
    }

    pub(crate) fn collapse_all(&mut self, kind: Option<FoldKind>) {
        let starts: Vec<usize> = self
            .regions
            .iter()
            .filter(|region| kind.is_none_or(|kind| region.kind == kind))
            .map(|region| region.start)
            .collect();
        self.collapsed.extend(starts);
        self.recompute();
    }

    pub(crate) fn expand_all(&mut self) {
        self.collapsed.clear();
        self.recompute();
    }

    /// Unfolds whatever hides `line`.
    pub(crate) fn reveal(&mut self, line: usize) -> bool {
        if !self.is_hidden(line) {
            return false;
        }
        let regions = &self.regions;
        self.collapsed.retain(|start| {
            regions
                .iter()
                .find(|region| region.start == *start)
                .is_none_or(|region| !(region.start < line && line <= region.end))
        });
        self.recompute();
        true
    }

    pub(crate) fn is_hidden(&self, line: usize) -> bool {
        self.hidden.iter().any(|range| range.contains(&line))
    }

    /// The display row of `line`; a hidden line maps to the row of its fold's first line.
    pub(crate) fn row_of(&self, line: usize) -> usize {
        let mut hidden_before = 0;
        for range in &self.hidden {
            if line < range.start {
                break;
            }
            if range.contains(&line) {
                return range.start - 1 - hidden_before;
            }
            hidden_before += range.len();
        }
        line - hidden_before
    }

    pub(crate) fn line_of(&self, row: usize) -> usize {
        let mut line = row;
        for range in &self.hidden {
            if range.start <= line {
                line += range.len();
            } else {
                break;
            }
        }
        line
    }

    pub(crate) fn row_count(&self, line_count: usize) -> usize {
        line_count - self.hidden.iter().map(std::ops::Range::len).sum::<usize>()
    }
}

const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DisplayLine {
    pub(crate) text: String,
    /// For each tab: its byte column in the buffer line and how many spaces replaced it.
    tabs: Vec<(usize, usize)>,
}

impl DisplayLine {
    /// Expands the tabs of `line`. At most `max_bytes` of the line are kept, so that a
    /// minified file cannot make shaping a single line arbitrarily slow.
    pub(crate) fn new(line: &str, max_bytes: usize) -> Self {
        let mut end = line.len().min(max_bytes);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let line = line.get(..end).unwrap_or("");
        if !line.contains('\t') {
            return Self {
                text: line.to_owned(),
                tabs: Vec::new(),
            };
        }
        let mut text = String::with_capacity(line.len() + TAB_WIDTH);
        let mut tabs = Vec::new();
        let mut column = 0;
        for (index, c) in line.char_indices() {
            if c == '\t' {
                let width = TAB_WIDTH - column % TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', width));
                tabs.push((index, width));
                column += width;
            } else {
                text.push(c);
                column += 1;
            }
        }
        Self { text, tabs }
    }

    pub(crate) fn to_display(&self, column: usize) -> usize {
        let extra: usize = self
            .tabs
            .iter()
            .take_while(|(at, _)| *at < column)
            .map(|(_, width)| width - 1)
            .sum();
        (column + extra).min(self.text.len())
    }

    /// Maps a byte column of the drawn text back to the buffer line. A position inside an
    /// expanded tab maps to the nearer side of the tab.
    pub(crate) fn to_buffer(&self, display_column: usize) -> usize {
        let mut extra = 0;
        for (at, width) in &self.tabs {
            let start = at + extra;
            if display_column <= start {
                break;
            }
            if display_column < start + width {
                let after_midpoint = display_column - start > width / 2;
                return at + usize::from(after_midpoint);
            }
            extra += width - 1;
        }
        display_column - extra
    }
}

/// Parses the text of the Go to Line popup: `line` or `line:column`, both one-based.
pub(crate) fn parse_line_column(text: &str) -> Option<(usize, Option<usize>)> {
    let (line, column) = match text.split_once(':') {
        Some((line, column)) => (line, Some(column)),
        None => (text, None),
    };
    let line = line.trim().parse().ok().filter(|line| *line >= 1)?;
    let column = match column.map(str::trim) {
        None | Some("") => None,
        Some(column) => Some(column.parse().ok().filter(|column| *column >= 1)?),
    };
    Some((line, column))
}

pub(crate) fn scroll_to_reveal(
    scroll_top: f64,
    visible_lines: f64,
    line: usize,
    margin: f64,
    line_count: usize,
) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "line numbers are far below 2^52"
    )]
    let (line, max_top) = (line as f64, (line_count as f64 - 1.0).max(0.0));
    let margin = margin.min((visible_lines - 1.0).max(0.0) / 2.0);
    let top = if line < scroll_top + margin {
        line - margin
    } else if line + 1.0 > scroll_top + visible_lines - margin {
        line + 1.0 + margin - visible_lines
    } else {
        scroll_top
    };
    top.clamp(0.0, max_top)
}

pub(crate) fn scroll_to_center(visible_lines: f64, line: usize, line_count: usize) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "line numbers are far below 2^52"
    )]
    let (line, max_top) = (line as f64, (line_count as f64 - 1.0).max(0.0));
    (line - (visible_lines - 1.0) / 2.0).clamp(0.0, max_top)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(text: &str) -> EditorModel {
        EditorModel::new(Buffer::new(text))
    }

    fn at(model: &EditorModel) -> (usize, usize) {
        let point = model.caret_point();
        (point.line.get(), point.column)
    }

    #[test]
    fn horizontal_moves_cross_lines() {
        let mut m = model("ab\ncd");
        m.move_to(ByteOffset(2), false);
        m.apply_motion(Motion::Right, false);
        assert_eq!(at(&m), (1, 0));
        m.apply_motion(Motion::Left, false);
        assert_eq!(at(&m), (0, 2));
        m.apply_motion(Motion::TextEnd, false);
        m.apply_motion(Motion::Right, false);
        assert_eq!(at(&m), (1, 2), "stops at the end");
        m.apply_motion(Motion::TextStart, false);
        m.apply_motion(Motion::Left, false);
        assert_eq!(at(&m), (0, 0), "stops at the start");
    }

    #[test]
    fn vertical_moves_keep_a_goal_column() {
        let mut m = model("long line here\nx\nanother long line");
        m.move_to(ByteOffset(9), false);
        m.apply_motion(Motion::Down, false);
        assert_eq!(at(&m), (1, 1), "clamped to the short line");
        m.apply_motion(Motion::Down, false);
        assert_eq!(at(&m), (2, 9), "returns to the goal column");
        m.apply_motion(Motion::Left, false);
        m.apply_motion(Motion::Up, false);
        m.apply_motion(Motion::Up, false);
        assert_eq!(at(&m), (0, 8), "a horizontal move resets the goal");
        m.apply_motion(Motion::Up, false);
        assert_eq!(at(&m), (0, 0), "up on the first line goes to the start");
        m.apply_motion(Motion::PageDown(50), false);
        assert_eq!(at(&m), (2, 8));
        m.apply_motion(Motion::Down, false);
        assert_eq!(at(&m), (2, 17), "down on the last line goes to the end");
        m.apply_motion(Motion::PageUp(1), false);
        assert_eq!(at(&m).0, 1);
    }

    #[test]
    fn vertical_moves_count_characters_not_bytes() {
        let mut m = model("ééé\nabc");
        m.move_to(ByteOffset(4), false);
        m.apply_motion(Motion::Down, false);
        assert_eq!(at(&m), (1, 2));
    }

    #[test]
    fn line_start_toggles_between_indent_and_column_zero() {
        let mut m = model("    indented");
        m.move_to(ByteOffset(8), false);
        m.apply_motion(Motion::LineStart, false);
        assert_eq!(at(&m), (0, 4));
        m.apply_motion(Motion::LineStart, false);
        assert_eq!(at(&m), (0, 0));
        m.apply_motion(Motion::LineStart, false);
        assert_eq!(at(&m), (0, 4));
        m.apply_motion(Motion::LineEnd, false);
        assert_eq!(at(&m), (0, 12));
    }

    #[test]
    fn word_moves() {
        let mut m = model("foo.bar(baz)\n  next");
        m.apply_motion(Motion::WordRight, false);
        assert_eq!(at(&m), (0, 3));
        m.apply_motion(Motion::WordRight, false);
        assert_eq!(at(&m), (0, 7));
        m.apply_motion(Motion::TextEnd, false);
        m.apply_motion(Motion::WordLeft, false);
        assert_eq!(at(&m), (1, 2));
        m.apply_motion(Motion::WordLeft, false);
        assert_eq!(at(&m), (1, 0));
        m.apply_motion(Motion::WordLeft, false);
        assert_eq!(at(&m), (0, 12), "wraps to the end of the previous line");
        m.apply_motion(Motion::WordRight, false);
        assert_eq!(at(&m), (1, 0), "wraps to the start of the next line");
        m.apply_motion(Motion::WordLeft, false);
        m.apply_motion(Motion::WordLeft, false);
        assert_eq!(at(&m), (0, 8), "start of baz");
    }

    #[test]
    fn selections_extend_and_collapse() {
        let mut m = model("hello world");
        m.move_to(ByteOffset(2), false);
        m.apply_motion(Motion::WordRight, true);
        assert_eq!(m.selected_text(), "llo");
        m.apply_motion(Motion::Left, false);
        assert_eq!(m.caret(), ByteOffset(2), "collapses to the start");
        m.apply_motion(Motion::LineEnd, true);
        m.apply_motion(Motion::Right, false);
        assert_eq!(m.caret(), ByteOffset(11), "collapses to the end");
        assert!(!m.has_selection());
        m.select_all();
        assert_eq!(m.selected_text(), "hello world");
    }

    #[test]
    fn word_and_line_selection() {
        let mut m = model("one two\nthree\n");
        assert!(m.select_word_at(ByteOffset(5)));
        assert_eq!(m.selected_text(), "two");
        m.select_line(LineIndex(0));
        assert_eq!(m.selected_text(), "one two\n");
        m.select_line(LineIndex(2));
        assert_eq!(m.selected_text(), "", "the last empty line");
        let none = |_: ByteRange| None;
        let mut blank = model("   ");
        blank.move_to(ByteOffset(1), false);
        assert!(!blank.extend_selection(none));
    }

    #[test]
    fn extend_and_shrink_selection_step_through_enclosing_ranges() {
        let mut m = model("call(one, two)");
        // A stand-in for the syntax tree: the arguments, then the whole call.
        let enclosing = |range: ByteRange| match (range.start.get(), range.end.get()) {
            (5, 8) => Some(ByteOffset(4)..ByteOffset(14)),
            (4, 14) => Some(ByteOffset(0)..ByteOffset(14)),
            _ => None,
        };
        m.move_to(ByteOffset(6), false);
        assert!(m.extend_selection(enclosing));
        assert_eq!(m.selected_text(), "one", "first the word");
        assert!(m.extend_selection(enclosing));
        assert_eq!(m.selected_text(), "(one, two)");
        assert!(m.extend_selection(enclosing));
        assert_eq!(m.selected_text(), "call(one, two)");
        assert!(
            !m.extend_selection(enclosing),
            "nothing encloses the whole text"
        );
        assert!(m.shrink_selection());
        assert_eq!(m.selected_text(), "(one, two)");
        assert!(m.shrink_selection());
        assert!(m.shrink_selection());
        assert!(!m.has_selection());
        assert_eq!(m.caret(), ByteOffset(6), "back where the caret was");
        assert!(!m.shrink_selection());

        m.move_to(ByteOffset(6), false);
        assert!(m.extend_selection(enclosing));
        m.move_to(ByteOffset(0), false);
        assert!(!m.shrink_selection(), "moving the caret forgets the steps");
    }

    fn region(start: usize, end: usize) -> FoldRegion {
        FoldRegion {
            header: start,
            start,
            end,
            kind: FoldKind::Code,
        }
    }

    #[test]
    fn folds_map_lines_to_rows() {
        let mut folds = Folds::default();
        folds.set_regions(vec![region(2, 5), region(3, 4), region(8, 9), region(8, 8)]);
        assert_eq!(folds.row_count(12), 12);
        assert!(folds.toggle(2));
        assert_eq!(folds.row_count(12), 9, "lines 3-5 are hidden");
        assert_eq!(folds.row_of(2), 2);
        assert_eq!(folds.row_of(4), 2, "a hidden line maps to its fold's row");
        assert_eq!(folds.row_of(6), 3);
        assert_eq!(folds.line_of(3), 6);
        assert!(folds.collapse_at(8));
        assert_eq!(folds.line_of(5), 8);
        assert_eq!(folds.line_of(6), 10, "line 9 is hidden too");
        assert!(folds.reveal(9));
        assert!(!folds.is_collapsed(8));
        assert!(
            folds.is_collapsed(2),
            "revealing one fold leaves the others"
        );
        assert!(
            folds.collapse_at(3),
            "the inner region inside an expanded outer one"
        );
        assert!(folds.expand_at(2));
        assert!(folds.is_hidden(4), "the inner fold stays collapsed");
        folds.expand_all();
        assert_eq!(folds.row_count(12), 12);
        folds.set_regions(vec![region(8, 9)]);
        assert_eq!(folds.region_starting(2), None);
    }

    #[test]
    fn vertical_moves_skip_folded_lines() {
        let mut m = model("a\nb {\nc\nd\n}\ne\n");
        m.folds.set_regions(vec![region(1, 4)]);
        m.folds.toggle(1);
        m.move_to(ByteOffset(2), false);
        m.apply_motion(Motion::Down, false);
        assert_eq!(
            m.caret_point().line.get(),
            5,
            "from the fold header to after the fold"
        );
        m.apply_motion(Motion::Up, false);
        assert_eq!(m.caret_point().line.get(), 1);
        m.move_to(m.buffer().line_start(LineIndex(3)), false);
        assert!(
            !m.folds.is_collapsed(1),
            "putting the caret inside a fold opens it"
        );
    }

    #[test]
    fn go_to_line_and_column() {
        let mut m = model("first\n  sécond\nthird");
        m.go_to(2, None);
        assert_eq!(at(&m), (1, 0));
        m.go_to(2, Some(5));
        assert_eq!(at(&m), (1, 5), "column 5 is after é (2 bytes)");
        m.go_to(99, Some(99));
        assert_eq!(at(&m), (2, 5), "clamps to the last line and its end");
        m.go_to(0, Some(0));
        assert_eq!(at(&m), (0, 0));
    }

    #[test]
    fn display_lines_expand_tabs_to_tab_stops() {
        let line = DisplayLine::new("\ta\tbc\t", 1000);
        assert_eq!(line.text, "    a   bc  ");
        assert_eq!(line.to_display(0), 0);
        assert_eq!(line.to_display(1), 4);
        assert_eq!(line.to_display(2), 5);
        assert_eq!(line.to_display(3), 8);
        assert_eq!(line.to_display(6), 12);
        assert_eq!(line.to_display(99), 12, "clamped");
        for column in 0..=6 {
            assert_eq!(line.to_buffer(line.to_display(column)), column);
        }
        assert_eq!(line.to_buffer(1), 0, "left half of a tab");
        assert_eq!(line.to_buffer(3), 1, "right half of a tab");
        assert_eq!(line.to_buffer(6), 2, "inside the second tab, left half");
    }

    #[test]
    fn display_lines_without_tabs_are_unchanged_and_long_lines_are_cut() {
        let line = DisplayLine::new("héllo", 1000);
        assert_eq!(line.text, "héllo");
        assert_eq!(line.to_display(3), 3);
        assert_eq!(line.to_buffer(3), 3);
        let cut = DisplayLine::new("ééé", 3);
        assert_eq!(cut.text, "é", "cut on a character boundary");
        assert_eq!(cut.to_display(6), 2);
    }

    #[test]
    fn parses_line_and_column() {
        assert_eq!(parse_line_column("12"), Some((12, None)));
        assert_eq!(parse_line_column(" 12 : 3 "), Some((12, Some(3))));
        assert_eq!(parse_line_column("12:"), Some((12, None)));
        assert_eq!(parse_line_column("0"), None);
        assert_eq!(parse_line_column("3:0"), None);
        assert_eq!(parse_line_column("abc"), None);
        assert_eq!(parse_line_column("3:x"), None);
        assert_eq!(parse_line_column(""), None);
        assert_eq!(parse_line_column("-1"), None);
    }

    #[test]
    fn reveal_scrolls_only_when_needed() {
        let reveal = |top, line| scroll_to_reveal(top, 10.0, line, 2.0, 100);
        assert!((reveal(20.0, 25) - 20.0).abs() < 1e-9, "already visible");
        assert!(
            (reveal(20.0, 21) - 19.0).abs() < 1e-9,
            "keeps a margin above"
        );
        assert!(
            (reveal(20.0, 29) - 22.0).abs() < 1e-9,
            "keeps a margin below"
        );
        assert!((reveal(20.0, 0) - 0.0).abs() < 1e-9);
        assert!((reveal(0.0, 99) - 92.0).abs() < 1e-9);
        assert!(
            (scroll_to_reveal(0.0, 1.0, 5, 3.0, 100) - 5.0).abs() < 1e-9,
            "tiny viewport"
        );
    }

    #[test]
    fn center_clamps_to_the_document() {
        assert!((scroll_to_center(11.0, 50, 100) - 45.0).abs() < 1e-9);
        assert!((scroll_to_center(11.0, 2, 100) - 0.0).abs() < 1e-9);
        assert!((scroll_to_center(11.0, 99, 100) - 94.0).abs() < 1e-9);
        assert!((scroll_to_center(11.0, 0, 0) - 0.0).abs() < 1e-9);
    }
}
