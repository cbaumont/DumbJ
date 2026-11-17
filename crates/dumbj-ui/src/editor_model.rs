use dumbj_text::disk::LineEnding;
use dumbj_text::{
    Buffer, ByteOffset, ByteRange, Carets, Edit, EditKind, History, LineIndex, Point, is_word_char,
};

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
    history: History,
    format: Format,
}

/// How the file is written, so that edits match what is already there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) line_ending: LineEnding,
    /// One level of indentation: a tab or a run of spaces.
    pub(crate) indent: String,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            line_ending: LineEnding::Lf,
            indent: " ".repeat(TAB_WIDTH),
        }
    }
}

impl Format {
    /// Reads the line ending and the indentation unit off the first lines of `buffer`.
    pub(crate) fn detect(buffer: &Buffer) -> Self {
        const SAMPLE_LINES: usize = 2000;
        let lines = buffer.line_count().min(SAMPLE_LINES);
        let sample_end = buffer.line_end(LineIndex(lines - 1));
        let end = buffer.next_char(buffer.next_char(sample_end));
        let sample = buffer.text(ByteOffset::ZERO..end).unwrap_or_default();
        let mut spaces: Option<usize> = None;
        let mut indent = None;
        for line in sample.lines() {
            if line.starts_with('\t') {
                indent = Some("\t".to_owned());
                break;
            }
            let width = line.len() - line.trim_start_matches(' ').len();
            if width >= 2 && width < line.len() {
                spaces = Some(spaces.map_or(width, |least| least.min(width)));
            }
        }
        Self {
            line_ending: LineEnding::detect(&sample),
            indent: indent.unwrap_or_else(|| " ".repeat(spaces.unwrap_or(TAB_WIDTH).min(8))),
        }
    }
}

/// Where `position` ends up after `removed` bytes at `offset` were replaced by `inserted`.
fn shifted(
    position: ByteOffset,
    offset: ByteOffset,
    removed: usize,
    inserted: usize,
) -> ByteOffset {
    if position <= offset {
        position
    } else if position.get() >= offset.get() + removed {
        ByteOffset(position.get() - removed + inserted)
    } else {
        ByteOffset(offset.get() + inserted)
    }
}

fn opens_block(c: char) -> bool {
    matches!(c, '{' | '(' | '[')
}

fn closes(open: char, close: char) -> bool {
    matches!((open, close), ('{', '}') | ('(', ')') | ('[', ']'))
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
        self.history.seal();
        self.caret = self.buffer.clamp(offset);
        self.folds
            .reveal(self.buffer.point_at(self.caret).line.get());
        if !extend {
            self.anchor = self.caret;
        }
        self.goal_column = None;
    }

    pub(crate) fn select(&mut self, range: ByteRange) {
        self.history.seal();
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

    pub(crate) fn set_format(&mut self, format: Format) {
        self.format = format;
    }

    pub(crate) fn is_modified(&self) -> bool {
        self.history.is_modified()
    }

    pub(crate) fn mark_saved(&mut self) {
        self.history.mark_saved();
    }

    /// Ends the current typing group, so the next edit is its own undo step.
    pub(crate) fn seal_history(&mut self) {
        self.history.seal();
    }

    /// Replaces the text with what is on disk now, as one undoable edit that counts as saved.
    pub(crate) fn reload(&mut self, text: &str) -> Vec<Edit> {
        let caret = self.caret.min(ByteOffset(text.len()));
        let mut caret = caret.get();
        while !text.is_char_boundary(caret) {
            caret -= 1;
        }
        let caret = ByteOffset(caret);
        let edits = self.replace(
            ByteOffset::ZERO..self.buffer.len(),
            text,
            EditKind::Other,
            caret..caret,
        );
        self.history.seal();
        self.history.mark_saved();
        edits
    }

    fn carets(&self) -> Carets {
        Carets {
            anchor: self.anchor,
            caret: self.caret,
        }
    }

    fn after_edit(&mut self, edit: &Edit) {
        self.folds.apply_edit(
            edit.start_point.line.get(),
            edit.old_end_point.line.get(),
            edit.new_end_point.line.get(),
        );
        self.goal_column = None;
        self.extended.clear();
    }

    fn restore(&mut self, carets: Carets) {
        self.anchor = self.buffer.clamp(carets.anchor);
        self.caret = self.buffer.clamp(carets.caret);
        self.folds
            .reveal(self.buffer.point_at(self.caret).line.get());
    }

    /// One edit; the selection becomes `select` (anchor first) afterwards.
    fn replace(
        &mut self,
        range: ByteRange,
        text: &str,
        kind: EditKind,
        select: ByteRange,
    ) -> Vec<Edit> {
        let after = Carets {
            anchor: select.start,
            caret: select.end,
        };
        let before = self.carets();
        let Ok(edit) = self
            .history
            .apply(&mut self.buffer, range, text, kind, before, after)
        else {
            return Vec::new();
        };
        self.after_edit(&edit);
        self.restore(after);
        vec![edit]
    }

    /// Several edits as one undo step. `edits` do not overlap and are sorted from the
    /// last in the buffer to the first, so that applying one leaves the others in place.
    /// The selection follows the text it was on.
    fn replace_many(&mut self, edits: &[(ByteRange, String)]) -> Vec<Edit> {
        let before = self.carets();
        let mut after = before;
        for (range, text) in edits {
            let removed = range.end.get() - range.start.get();
            after.anchor = shifted(after.anchor, range.start, removed, text.len());
            after.caret = shifted(after.caret, range.start, removed, text.len());
        }
        let mut applied = Vec::new();
        self.history.seal();
        self.history.begin_group();
        for (range, text) in edits {
            if let Ok(edit) = self.history.apply(
                &mut self.buffer,
                range.clone(),
                text,
                EditKind::Other,
                before,
                after,
            ) {
                self.after_edit(&edit);
                applied.push(edit);
            }
        }
        self.history.end_group();
        self.restore(after);
        applied
    }

    /// Types `text` over the selection.
    pub(crate) fn insert(&mut self, text: &str) -> Vec<Edit> {
        self.replace_range(self.selection(), text)
    }

    /// Replaces `range`, which need not be the selection (an input method does this).
    pub(crate) fn replace_range(&mut self, range: ByteRange, text: &str) -> Vec<Edit> {
        let range = self.buffer.clamp(range.start)..self.buffer.clamp(range.end);
        let end = ByteOffset(range.start.get() + text.len());
        self.replace(range, text, EditKind::Insert, end..end)
    }

    /// Inserts clipboard text, with its line breaks turned into the file's.
    pub(crate) fn paste(&mut self, text: &str) -> Vec<Edit> {
        let ending = self.format.line_ending.as_str();
        let text = text.replace("\r\n", "\n").replace('\n', ending);
        let range = self.selection();
        let end = ByteOffset(range.start.get() + text.len());
        self.history.seal();
        let edits = self.replace(range, &text, EditKind::Other, end..end);
        self.history.seal();
        edits
    }

    pub(crate) fn delete_selection(&mut self) -> Vec<Edit> {
        let range = self.selection();
        if range.is_empty() {
            return Vec::new();
        }
        let start = range.start;
        self.replace(range, "", EditKind::Other, start..start)
    }

    pub(crate) fn backspace(&mut self) -> Vec<Edit> {
        if self.has_selection() {
            return self.delete_selection();
        }
        let point = self.caret_point();
        let start = if point.column == 0 && point.line.get() > 0 {
            self.buffer.line_end(LineIndex(point.line.get() - 1))
        } else {
            self.buffer.prev_char(self.caret)
        };
        self.replace(
            start..self.caret,
            "",
            EditKind::DeleteBackward,
            start..start,
        )
    }

    pub(crate) fn delete_forward(&mut self) -> Vec<Edit> {
        if self.has_selection() {
            return self.delete_selection();
        }
        let point = self.caret_point();
        let next_line = LineIndex(point.line.get() + 1);
        let end = if self.caret == self.buffer.line_end(point.line)
            && next_line.get() < self.buffer.line_count()
        {
            self.buffer.line_start(next_line)
        } else {
            self.buffer.next_char(self.caret)
        };
        let caret = self.caret;
        self.replace(caret..end, "", EditKind::DeleteForward, caret..caret)
    }

    /// Breaks the line, keeping its indentation and adding a level after an opening bracket.
    pub(crate) fn newline(&mut self) -> Vec<Edit> {
        let range = self.selection();
        let point = self.buffer.point_at(range.start);
        let line = self.buffer.line(point.line);
        let before = line.get(..point.column).unwrap_or(&line);
        let base: String = before
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let ending = self.format.line_ending.as_str();
        let opener = before
            .trim_end()
            .chars()
            .next_back()
            .filter(|c| opens_block(*c));
        let after = self.buffer.line(self.buffer.point_at(range.end).line);
        let end_column = self.buffer.point_at(range.end).column;
        let closer = after
            .get(end_column..)
            .and_then(|rest| rest.trim_start().chars().next());
        let mut text = format!("{ending}{base}");
        if opener.is_some() {
            text.push_str(&self.format.indent);
        }
        let caret = ByteOffset(range.start.get() + text.len());
        if opener
            .zip(closer)
            .is_some_and(|(open, close)| closes(open, close))
        {
            text.push_str(ending);
            text.push_str(&base);
        }
        self.replace(range, &text, EditKind::Insert, caret..caret)
    }

    /// The lines the selection touches; a selection ending at the start of a line does
    /// not include that line.
    pub(crate) fn selected_lines(&self) -> std::ops::RangeInclusive<usize> {
        let selection = self.selection();
        let first = self.buffer.point_at(selection.start).line.get();
        let end = self.buffer.point_at(selection.end);
        let last = if end.column == 0 && end.line.get() > first {
            end.line.get() - 1
        } else {
            end.line.get()
        };
        first..=last
    }

    /// Tab: indents the selected lines, or moves the caret to the next indentation stop.
    pub(crate) fn indent(&mut self) -> Vec<Edit> {
        let lines = self.selected_lines();
        if !self.has_selection() || lines.start() == lines.end() {
            let unit = &self.format.indent;
            let text = if unit.starts_with('\t') {
                unit.clone()
            } else {
                let start = self.buffer.point_at(self.selection().start);
                let column = self.buffer.char_column(start);
                " ".repeat(unit.len() - column % unit.len())
            };
            return self.insert(&text);
        }
        let edits: Vec<(ByteRange, String)> = lines
            .rev()
            .filter(|line| self.buffer.line_len(LineIndex(*line)) > 0)
            .map(|line| {
                let start = self.buffer.line_start(LineIndex(line));
                (start..start, self.format.indent.clone())
            })
            .collect();
        self.replace_many(&edits)
    }

    /// Shift+Tab: removes one level of indentation from the selected lines.
    pub(crate) fn unindent(&mut self) -> Vec<Edit> {
        let width = self.format.indent.len();
        let edits: Vec<(ByteRange, String)> = self
            .selected_lines()
            .rev()
            .filter_map(|line| {
                let text = self.buffer.line(LineIndex(line));
                let remove = if text.starts_with('\t') {
                    1
                } else {
                    text.bytes().take(width).take_while(|b| *b == b' ').count()
                };
                let start = self.buffer.line_start(LineIndex(line));
                (remove > 0).then(|| (start..ByteOffset(start.get() + remove), String::new()))
            })
            .collect();
        self.replace_many(&edits)
    }

    /// The selected lines with their line breaks, as a range of whole lines.
    fn line_block(&self, lines: &std::ops::RangeInclusive<usize>) -> ByteRange {
        let start = self.buffer.line_start(LineIndex(*lines.start()));
        let next = LineIndex(lines.end() + 1);
        if next.get() < self.buffer.line_count() {
            start..self.buffer.line_start(next)
        } else {
            start..self.buffer.len()
        }
    }

    pub(crate) fn delete_line(&mut self) -> Vec<Edit> {
        let lines = self.selected_lines();
        let column = self.buffer.char_column(self.caret_point());
        let mut range = self.line_block(&lines);
        let last_line = lines.end() + 1 >= self.buffer.line_count();
        if last_line && *lines.start() > 0 {
            range.start = self.buffer.line_end(LineIndex(lines.start() - 1));
        }
        let start = range.start;
        let edits = self.replace(range, "", EditKind::Other, start..start);
        let line = self.buffer.point_at(start).line;
        self.restore_column(line, column);
        edits
    }

    fn restore_column(&mut self, line: LineIndex, column: usize) {
        let offset = self.offset_at_char_column(line, column);
        self.anchor = offset;
        self.caret = offset;
    }

    /// Duplicates the selection after itself, or the caret's line below itself.
    pub(crate) fn duplicate(&mut self) -> Vec<Edit> {
        if self.has_selection() {
            let selection = self.selection();
            let text = self.selected_text();
            let end = ByteOffset(selection.end.get() + text.len());
            return self.replace(
                selection.end..selection.end,
                &text,
                EditKind::Other,
                selection.end..end,
            );
        }
        let line = self.caret_point().line;
        let text = self.buffer.line(line).into_owned();
        let end = self.buffer.line_end(line);
        let insert = format!("{}{text}", self.format.line_ending.as_str());
        let caret = ByteOffset(
            self.caret.get() - self.buffer.line_start(line).get()
                + end.get()
                + self.format.line_ending.as_str().len(),
        );
        self.replace(end..end, &insert, EditKind::Other, caret..caret)
    }

    /// Moves the selected lines one line up or down, with the selection.
    pub(crate) fn move_lines(&mut self, down: bool) -> Vec<Edit> {
        let lines = self.selected_lines();
        let last = self.buffer.line_count() - 1;
        if (down && *lines.end() >= last) || (!down && *lines.start() == 0) {
            return Vec::new();
        }
        let ending = self.format.line_ending.as_str();
        let content = |from: usize, to: usize| {
            self.buffer
                .text(self.buffer.line_start(LineIndex(from))..self.buffer.line_end(LineIndex(to)))
                .unwrap_or_default()
        };
        let block = content(*lines.start(), *lines.end());
        let (first, last_line, text, moved_by) = if down {
            let other = content(lines.end() + 1, lines.end() + 1);
            let by = other.len() + ending.len();
            (
                *lines.start(),
                lines.end() + 1,
                format!("{other}{ending}{block}"),
                by.cast_signed(),
            )
        } else {
            let other = content(lines.start() - 1, lines.start() - 1);
            let by = other.len() + ending.len();
            (
                lines.start() - 1,
                *lines.end(),
                format!("{block}{ending}{other}"),
                -by.cast_signed(),
            )
        };
        let range =
            self.buffer.line_start(LineIndex(first))..self.buffer.line_end(LineIndex(last_line));
        let moved = |offset: ByteOffset| ByteOffset(offset.get().saturating_add_signed(moved_by));
        let select = moved(self.anchor)..moved(self.caret);
        self.replace(range, &text, EditKind::Other, select)
    }

    /// Comments the selected lines with `prefix`, or uncomments them if all are commented.
    /// Without a selection the caret then moves down a line, as IDEA's does.
    pub(crate) fn toggle_line_comment(&mut self, prefix: &str) -> Vec<Edit> {
        let lines: Vec<usize> = self
            .selected_lines()
            .filter(|line| !self.buffer.line(LineIndex(*line)).trim().is_empty())
            .collect();
        let indent_of = |line: usize| {
            let text = self.buffer.line(LineIndex(line));
            text.len() - text.trim_start().len()
        };
        let commented = !lines.is_empty()
            && lines.iter().all(|line| {
                self.buffer
                    .line(LineIndex(*line))
                    .trim_start()
                    .starts_with(prefix)
            });
        let column = lines.iter().map(|line| indent_of(*line)).min().unwrap_or(0);
        let edits: Vec<(ByteRange, String)> = lines
            .iter()
            .rev()
            .map(|line| {
                let start = self.buffer.line_start(LineIndex(*line)).get();
                if commented {
                    let at = start + indent_of(*line);
                    let text = self.buffer.line(LineIndex(*line));
                    let spaced = text
                        .get(indent_of(*line) + prefix.len()..)
                        .is_some_and(|rest| rest.starts_with(' '));
                    let end = at + prefix.len() + usize::from(spaced);
                    (ByteOffset(at)..ByteOffset(end), String::new())
                } else {
                    let at = ByteOffset(start + column);
                    (at..at, format!("{prefix} "))
                }
            })
            .collect();
        let had_selection = self.has_selection();
        let applied = self.replace_many(&edits);
        if !had_selection {
            self.apply_motion(Motion::Down, false);
        }
        applied
    }

    pub(crate) fn undo(&mut self) -> Vec<Edit> {
        let Some((edits, carets)) = self.history.undo(&mut self.buffer) else {
            return Vec::new();
        };
        for edit in &edits {
            self.after_edit(edit);
        }
        self.restore(carets);
        edits
    }

    pub(crate) fn redo(&mut self) -> Vec<Edit> {
        let Some((edits, carets)) = self.history.redo(&mut self.buffer) else {
            return Vec::new();
        };
        for edit in &edits {
            self.after_edit(edit);
        }
        self.restore(carets);
        edits
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
            .skip_while(|(_, c)| !is_word_char(*c))
            .take_while(|(_, c)| is_word_char(*c))
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
            .skip_while(|(_, c)| !is_word_char(*c))
            .find(|(_, c)| !is_word_char(*c))
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

    /// Follows an edit that replaced lines `start..=old_end` with lines `start..=new_end`:
    /// regions below move with their text, regions around the edit grow or shrink, and
    /// regions that began inside the replaced lines are dropped until the next parse.
    pub(crate) fn apply_edit(&mut self, start: usize, old_end: usize, new_end: usize) {
        if old_end == new_end {
            return;
        }
        let moved = |line: usize| line - old_end + new_end;
        let collapsed = std::mem::take(&mut self.collapsed);
        let mut regions = Vec::with_capacity(self.regions.len());
        for region in std::mem::take(&mut self.regions) {
            let was_collapsed = collapsed.contains(&region.start);
            let region = if region.start > old_end {
                Some(FoldRegion {
                    header: if region.header > old_end {
                        moved(region.header)
                    } else {
                        region.header.min(start)
                    },
                    start: moved(region.start),
                    end: moved(region.end),
                    ..region
                })
            } else if region.end < start {
                Some(region)
            } else if region.start <= start && region.end > old_end {
                Some(FoldRegion {
                    end: moved(region.end),
                    ..region
                })
            } else if region.start < start {
                Some(FoldRegion {
                    end: start,
                    ..region
                })
            } else {
                None
            };
            if let Some(region) = region.filter(|region| region.end > region.start) {
                if was_collapsed {
                    self.collapsed.insert(region.start);
                }
                regions.push(region);
            }
        }
        self.regions = regions;
        self.recompute();
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
        line_count
            .saturating_sub(self.hidden.iter().map(std::ops::Range::len).sum::<usize>())
            .max(1)
    }
}

const TAB_WIDTH: usize = 4;

/// Text drawn in place of buffer text: an expanded tab, or an inlay hint, which replaces
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Insert {
    /// Byte column in the buffer line.
    at: usize,
    replaced: usize,
    width: usize,
}

impl Insert {
    fn is_inlay(&self) -> bool {
        self.replaced == 0
    }

    /// Whether the insert is drawn before buffer column `column`. A caret at an inlay's
    /// column is drawn after the inlay.
    fn before(&self, column: usize) -> bool {
        if self.is_inlay() {
            self.at <= column
        } else {
            self.at < column
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DisplayLine {
    pub(crate) text: String,
    inserts: Vec<Insert>,
}

impl DisplayLine {
    /// Expands the tabs of `line`. At most `max_bytes` of the line are kept, so that a
    /// minified file cannot make shaping a single line arbitrarily slow.
    pub(crate) fn new(line: &str, max_bytes: usize) -> Self {
        Self::with_inlays(line, max_bytes, &[])
    }

    /// Like `new`, and draws each inlay's text before the byte column it is attached to.
    /// `inlays` are sorted by column.
    pub(crate) fn with_inlays(line: &str, max_bytes: usize, inlays: &[(usize, &str)]) -> Self {
        let mut end = line.len().min(max_bytes);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        let line = line.get(..end).unwrap_or("");
        if !line.contains('\t') && inlays.is_empty() {
            return Self {
                text: line.to_owned(),
                inserts: Vec::new(),
            };
        }
        let mut text = String::with_capacity(line.len() + TAB_WIDTH);
        let mut inserts = Vec::new();
        let mut column = 0;
        let mut pending = inlays.iter().filter(|(at, _)| *at <= line.len()).peekable();
        let mut put_inlays = |index: usize, text: &mut String, column: &mut usize| {
            while let Some((at, label)) = pending.next_if(|(at, _)| *at <= index) {
                inserts.push(Insert {
                    at: *at,
                    replaced: 0,
                    width: label.len(),
                });
                text.push_str(label);
                *column += label.chars().count();
            }
        };
        let mut tabs = Vec::new();
        for (index, c) in line.char_indices() {
            put_inlays(index, &mut text, &mut column);
            if c == '\t' {
                let width = TAB_WIDTH - column % TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', width));
                tabs.push(Insert {
                    at: index,
                    replaced: 1,
                    width,
                });
                column += width;
            } else {
                text.push(c);
                column += 1;
            }
        }
        put_inlays(line.len(), &mut text, &mut column);
        inserts.extend(tabs);
        inserts.sort_by_key(|insert| (insert.at, !insert.is_inlay()));
        Self { text, inserts }
    }

    pub(crate) fn to_display(&self, column: usize) -> usize {
        let extra: usize = self
            .inserts
            .iter()
            .take_while(|insert| insert.before(column))
            .map(|insert| insert.width - insert.replaced)
            .sum();
        (column + extra).min(self.text.len())
    }

    /// Maps a byte column of the drawn text back to the buffer line. A position inside an
    /// expanded tab maps to the nearer side of the tab, one inside an inlay to the column
    /// the inlay is attached to.
    pub(crate) fn to_buffer(&self, display_column: usize) -> usize {
        let mut extra = 0;
        for insert in &self.inserts {
            let start = insert.at + extra;
            if display_column <= start {
                break;
            }
            if display_column < start + insert.width {
                if insert.is_inlay() {
                    return insert.at;
                }
                let after_midpoint = display_column - start > insert.width / 2;
                return insert.at + usize::from(after_midpoint);
            }
            extra += insert.width - insert.replaced;
        }
        display_column - extra
    }

    /// Where the inlays are in the drawn text.
    pub(crate) fn inlay_ranges(&self) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        let mut extra = 0;
        self.inserts.iter().filter_map(move |insert| {
            let start = insert.at + extra;
            extra += insert.width - insert.replaced;
            insert.is_inlay().then_some(start..start + insert.width)
        })
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
    fn inlays_are_drawn_before_their_column_and_map_back_to_it() {
        let line = DisplayLine::with_inlays("f(1, \tx)", 1000, &[(2, "a: "), (5, "b: ")]);
        assert_eq!(line.text, "f(a: 1, b:  x)");
        assert_eq!(
            line.to_display(2),
            5,
            "a caret at the inlay's column is drawn after it"
        );
        assert_eq!(line.to_display(1), 1);
        assert_eq!(line.to_display(6), 12);
        assert_eq!(line.to_buffer(3), 2, "inside an inlay");
        for column in 0..=7 {
            assert_eq!(line.to_buffer(line.to_display(column)), column);
        }
        let ranges: Vec<_> = line.inlay_ranges().collect();
        assert_eq!(ranges, [2..5, 8..11]);
        let end = DisplayLine::with_inlays("g()", 1000, &[(3, " : Int")]);
        assert_eq!(end.text, "g() : Int");
        assert_eq!(end.inlay_ranges().next(), Some(3..9));
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
    /// Runs `edit` on `text`, where `|` marks the caret (or `[` and `]` the selection),
    /// and renders the result the same way.
    fn edited(text: &str, edit: impl FnOnce(&mut EditorModel)) -> String {
        let mut model = marked(text);
        edit(&mut model);
        render(&model)
    }

    fn marked(text: &str) -> EditorModel {
        let plain: String = text.chars().filter(|c| !"|[]".contains(*c)).collect();
        let mut model = model(&plain);
        if let Some(caret) = text.find('|') {
            model.move_to(ByteOffset(caret), false);
        } else {
            let start = text.find('[').unwrap();
            let end = text.find(']').unwrap() - 1;
            model.select(ByteOffset(start)..ByteOffset(end));
        }
        model
    }

    fn render(model: &EditorModel) -> String {
        let mut text = model.buffer().to_string();
        let selection = model.selection();
        if selection.is_empty() {
            text.insert(selection.start.get(), '|');
        } else {
            text.insert(selection.end.get(), ']');
            text.insert(selection.start.get(), '[');
        }
        text
    }

    #[test]
    fn typing_replaces_the_selection() {
        assert_eq!(edited("a|c", |m| drop(m.insert("b"))), "ab|c");
        assert_eq!(edited("a[xyz]c", |m| drop(m.insert("b"))), "ab|c");
        assert_eq!(edited("a|c", |m| drop(m.paste("1\r\n2"))), "a1\n2|c");
    }

    #[test]
    fn backspace_and_delete_cross_line_breaks() {
        assert_eq!(edited("ab|c", |m| drop(m.backspace())), "a|c");
        assert_eq!(edited("a\n|b", |m| drop(m.backspace())), "a|b");
        assert_eq!(edited("a\r\n|b", |m| drop(m.backspace())), "a|b");
        assert_eq!(edited("|a", |m| drop(m.backspace())), "|a");
        assert_eq!(edited("a|\r\nb", |m| drop(m.delete_forward())), "a|b");
        assert_eq!(edited("a|é", |m| drop(m.delete_forward())), "a|");
        assert_eq!(edited("a[bc]d", |m| drop(m.delete_forward())), "a|d");
    }

    #[test]
    fn enter_keeps_the_indentation_and_opens_blocks() {
        assert_eq!(edited("  a|b", |m| drop(m.newline())), "  a\n  |b");
        assert_eq!(edited("  f {|", |m| drop(m.newline())), "  f {\n      |");
        assert_eq!(
            edited("  f {|}", |m| drop(m.newline())),
            "  f {\n      |\n  }"
        );
        assert_eq!(edited("\tx|", |m| drop(m.newline())), "\tx\n\t|");
    }

    #[test]
    fn enter_uses_the_files_line_ending() {
        let mut model = marked("a|");
        model.set_format(Format {
            line_ending: LineEnding::CrLf,
            indent: "\t".to_owned(),
        });
        model.newline();
        model.indent();
        assert_eq!(render(&model), "a\r\n\t|");
    }

    #[test]
    fn tab_indents_to_the_next_stop_or_the_selected_lines() {
        assert_eq!(edited("a|", |m| drop(m.indent())), "a   |");
        assert_eq!(
            edited("[a\n\nb]\nc", |m| drop(m.indent())),
            "[    a\n\n    b]\nc"
        );
        assert_eq!(
            edited("    a\n  [b\n      c\n]d", |m| drop(m.unindent())),
            "    a\n[b\n  c\n]d"
        );
        assert_eq!(edited("  |a", |m| drop(m.unindent())), "|a");
    }

    #[test]
    fn line_actions() {
        assert_eq!(edited("a\nb|b\nc", |m| drop(m.delete_line())), "a\nc|");
        assert_eq!(edited("a\nb|b", |m| drop(m.delete_line())), "a|");
        assert_eq!(edited("a|b", |m| drop(m.delete_line())), "|");
        assert_eq!(
            edited("a\nb|b\nc", |m| drop(m.duplicate())),
            "a\nbb\nb|b\nc"
        );
        assert_eq!(edited("a[bc]d", |m| drop(m.duplicate())), "abc[bc]d");
        assert_eq!(
            edited("a\nb|b\nc", |m| drop(m.move_lines(true))),
            "a\nc\nb|b"
        );
        assert_eq!(
            edited("a\nb|b\nc", |m| drop(m.move_lines(false))),
            "b|b\na\nc"
        );
        assert_eq!(edited("a|\nb", |m| drop(m.move_lines(false))), "a|\nb");
        assert_eq!(
            edited("a\n[b\nc]c\nd", |m| drop(m.move_lines(true))),
            "a\nd\n[b\nc]c"
        );
    }

    #[test]
    fn line_comments_toggle() {
        assert_eq!(
            edited("  a|\n  b", |m| drop(m.toggle_line_comment("//"))),
            "  // a\n  b|"
        );
        assert_eq!(
            edited("[  a\n\n    b]", |m| drop(m.toggle_line_comment("//"))),
            "[  // a\n\n  //   b]"
        );
        assert_eq!(
            edited("[  // a\n  //b]", |m| drop(m.toggle_line_comment("//"))),
            "[  a\n  b]"
        );
    }

    #[test]
    fn undo_and_redo_restore_text_and_carets() {
        let mut model = marked("x|");
        for c in ["a", "b", " ", "c"] {
            model.insert(c);
        }
        model.newline();
        assert_eq!(render(&model), "xab c\n|");
        assert!(model.is_modified());
        model.undo();
        assert_eq!(render(&model), "xab c|");
        model.undo();
        assert_eq!(render(&model), "xab |");
        model.undo();
        assert_eq!(render(&model), "x|");
        assert!(!model.is_modified());
        assert_eq!(model.undo(), []);
        model.redo();
        model.redo();
        assert_eq!(render(&model), "xab c|");
        model.mark_saved();
        assert!(!model.is_modified());
    }

    #[test]
    fn moving_the_caret_splits_undo_steps() {
        let mut model = marked("|");
        model.insert("a");
        model.apply_motion(Motion::Left, false);
        model.apply_motion(Motion::Right, false);
        model.insert("b");
        model.undo();
        assert_eq!(render(&model), "a|");
    }

    #[test]
    fn a_grouped_edit_is_one_undo_step() {
        let mut model = marked("[a\nb]");
        model.indent();
        model.undo();
        assert_eq!(render(&model), "[a\nb]");
    }

    #[test]
    fn reload_counts_as_saved_and_can_be_undone() {
        let mut model = marked("one|");
        model.reload("three");
        assert_eq!(render(&model), "thr|ee");
        assert!(!model.is_modified());
        model.undo();
        assert_eq!(render(&model), "one|");
        assert!(model.is_modified());
    }

    #[test]
    fn detects_the_format() {
        let format = Format::detect(&Buffer::new("a {\r\n  b\r\n    c\r\n}\r\n"));
        assert_eq!(format.line_ending, LineEnding::CrLf);
        assert_eq!(format.indent, "  ");
        assert_eq!(Format::detect(&Buffer::new("a\n\tb\n")).indent, "\t");
        assert_eq!(Format::detect(&Buffer::new("")), Format::default());
    }

    #[test]
    fn folds_follow_edits() {
        let mut folds = Folds::default();
        folds.set_regions(vec![region(1, 3), region(5, 8), region(6, 7)]);
        folds.toggle(5);
        folds.apply_edit(0, 0, 2);
        assert!(folds.region_starting(3).is_some());
        assert!(folds.is_collapsed(7));
        assert_eq!(folds.row_of(10), 7);
        folds.apply_edit(8, 9, 8);
        assert_eq!(folds.region_starting(7).map(|r| r.end), Some(9));
        assert!(folds.region_starting(8).is_none());
        folds.apply_edit(0, 20, 0);
        assert_eq!(folds.row_count(1), 1);
    }
}
