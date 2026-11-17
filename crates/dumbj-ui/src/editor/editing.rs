use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dumbj_actions::Action;
use dumbj_text::disk::{self, FileStamp};
use dumbj_text::{ByteOffset, ByteRange, Edit, LineIndex, Point as TextPoint};
use gpui::{
    AppContext, Bounds, ClipboardItem, Context, EntityInputHandler, Pixels, Point, UTF16Selection,
    Window, point,
};

use crate::line_edit::utf16_to_byte;

use super::{EditorView, LINE_HEIGHT, Reveal, fold_regions};

const IDLE_SAVE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineChange {
    pub(crate) start: usize,
    pub(crate) old_end: usize,
    pub(crate) new_end: usize,
}

pub(crate) enum EditorEvent {
    Edited(Vec<LineChange>),
    StatusChanged,
    Synced,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl EditorView {
    pub(crate) fn is_modified(&self) -> bool {
        self.model.is_modified()
    }

    pub(crate) fn has_conflict(&self) -> bool {
        self.conflict.is_some()
    }

    pub(super) fn handle_edit_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let is_edit = matches!(
            action,
            Action::Backspace
                | Action::Delete
                | Action::Enter
                | Action::Tab
                | Action::Unindent
                | Action::Cut
                | Action::Paste
                | Action::Undo
                | Action::Redo
                | Action::DuplicateLine
                | Action::DeleteLine
                | Action::MoveLineUp
                | Action::MoveLineDown
                | Action::ToggleLineComment
        );
        if !is_edit {
            return false;
        }
        if self.read_only || !self.is_ready() {
            return true;
        }
        self.marked = None;
        let edits = match action {
            Action::Backspace => self.model.backspace(),
            Action::Delete => self.model.delete_forward(),
            Action::Enter => self.model.newline(),
            Action::Tab => self.model.indent(),
            Action::Unindent => self.model.unindent(),
            Action::Cut => {
                let selected = self.model.selected_text();
                if selected.is_empty() {
                    return true;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(selected));
                self.model.delete_selection()
            }
            Action::Paste => match cx.read_from_clipboard().and_then(|item| item.text()) {
                Some(text) => self.model.paste(&text),
                None => Vec::new(),
            },
            Action::Undo => self.model.undo(),
            Action::Redo => self.model.redo(),
            Action::DuplicateLine => self.model.duplicate(),
            Action::DeleteLine => self.model.delete_line(),
            Action::MoveLineUp => self.model.move_lines(false),
            Action::MoveLineDown => self.model.move_lines(true),
            Action::ToggleLineComment => {
                match self.language.and_then(dumbj_syntax::Language::line_comment) {
                    Some(prefix) => self.model.toggle_line_comment(prefix),
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        self.edited(&edits, cx);
        true
    }

    pub(super) fn edited(&mut self, edits: &[Edit], cx: &mut Context<Self>) {
        self.reveal = Some(Reveal::Minimal);
        cx.notify();
        if edits.is_empty() {
            return;
        }
        if let Some(tree) = self.syntax.as_mut() {
            for edit in edits {
                tree.edit(edit, self.model.buffer());
            }
        }
        self.shift_server_marks(edits);
        for icon in &mut self.gutter_icons {
            for edit in edits {
                if icon.offset >= edit.old_end {
                    let moved = icon.offset.get() - edit.old_end.get() + edit.new_end.get();
                    icon.offset = ByteOffset(moved);
                } else if icon.offset > edit.start {
                    icon.offset = edit.start;
                }
            }
        }
        self.usages = Arc::default();
        self.usages_at = None;
        let query = std::mem::take(&mut self.search.query);
        self.set_search_query(&query, cx);
        self.start_reparse(cx);
        self.changes_edited(cx);
        self.schedule_idle_save(cx);
        cx.emit(EditorEvent::Edited(
            edits
                .iter()
                .map(|edit| LineChange {
                    start: edit.start_point.line.get(),
                    old_end: edit.old_end_point.line.get(),
                    new_end: edit.new_end_point.line.get(),
                })
                .collect(),
        ));
        cx.emit(EditorEvent::StatusChanged);
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        self.reparse_cancel.store(true, Ordering::Relaxed);
        let Some(tree) = self.syntax.clone() else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.reparse_cancel = cancel.clone();
        let buffer = self.model.buffer().clone();
        cx.spawn(async move |this, cx| {
            let version = buffer.version();
            let parsed = cx
                .background_spawn(async move {
                    let tree =
                        dumbj_syntax::reparse(&tree, &buffer, || cancel.load(Ordering::Relaxed))
                            .ok()
                            .flatten()?;
                    let folds = fold_regions(&tree, &buffer);
                    Some((tree, folds))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some((tree, folds)) = parsed else { return };
                if this.model.buffer().version() != version {
                    return;
                }
                this.syntax = Some(tree);
                this.highlights = None;
                this.model.folds.set_regions(folds);
                cx.notify();
            });
        })
        .detach();
    }

    fn schedule_idle_save(&mut self, cx: &mut Context<Self>) {
        self.idle_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(IDLE_SAVE).await;
            let _ = this.update(cx, Self::save);
        }));
    }

    /// Refuses to overwrite a file that changed on disk since it was read.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if self.read_only
            || self.scratch
            || self.saving
            || self.conflict.is_some()
            || !self.model.is_modified()
        {
            return;
        }
        self.model.seal_history();
        self.saving = true;
        let (path, buffer, known) = (self.path.clone(), self.model.buffer().clone(), self.stamp);
        cx.spawn(async move |this, cx| {
            let version = buffer.version();
            let written = cx
                .background_spawn(async move {
                    let on_disk = FileStamp::of(&path).ok();
                    if on_disk.is_some() && on_disk != known {
                        return Err(on_disk);
                    }
                    disk::write_atomic(&path, buffer.to_string().as_bytes()).map_err(|error| {
                        tracing::error!(%error, path = %path.display(), "cannot save");
                        None
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match written {
                    Ok(stamp) => {
                        this.stamp = Some(stamp);
                        if this.model.buffer().version() == version {
                            this.model.mark_saved();
                        } else {
                            this.schedule_idle_save(cx);
                        }
                        cx.emit(EditorEvent::Synced);
                    }
                    Err(Some(on_disk)) => this.conflict = Some(on_disk),
                    Err(None) => {}
                }
                cx.emit(EditorEvent::StatusChanged);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn save_blocking(&mut self) {
        if self.read_only || self.scratch || self.conflict.is_some() || !self.model.is_modified() {
            return;
        }
        if FileStamp::of(&self.path).is_ok_and(|stamp| Some(stamp) != self.stamp) {
            tracing::warn!(path = %self.path.display(), "not saved: the file changed on disk");
            return;
        }
        match disk::write_atomic(&self.path, self.model.buffer().to_string().as_bytes()) {
            Ok(stamp) => {
                self.stamp = Some(stamp);
                self.model.mark_saved();
            }
            Err(error) => tracing::error!(%error, path = %self.path.display(), "cannot save"),
        }
    }

    pub(crate) fn check_disk(&mut self, cx: &mut Context<Self>) {
        if self.read_only || self.scratch || self.saving || !self.is_ready() {
            return;
        }
        let (path, known) = (self.path.clone(), self.stamp);
        cx.spawn(async move |this, cx| {
            let on_disk = cx
                .background_spawn(async move { FileStamp::of(&path).ok() })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(on_disk) = on_disk.filter(|stamp| Some(*stamp) != known) else {
                    return;
                };
                if this.saving || this.stamp != known {
                    return;
                }
                if this.model.is_modified() {
                    this.conflict = Some(on_disk);
                    cx.emit(EditorEvent::StatusChanged);
                    cx.notify();
                } else {
                    this.reload_from_disk(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_spawn(async move {
                    let bytes = std::fs::read(&path).ok()?;
                    let stamp = FileStamp::of(&path).ok()?;
                    Some((String::from_utf8(bytes).ok()?, stamp))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some((text, stamp)) = read else { return };
                this.stamp = Some(stamp);
                this.conflict = None;
                this.marked = None;
                let edits = this.model.reload(&text);
                this.edited(&edits, cx);
                this.idle_save = None;
                cx.emit(EditorEvent::Synced);
                cx.emit(EditorEvent::StatusChanged);
            });
        })
        .detach();
    }

    pub(crate) fn keep_local_changes(&mut self, cx: &mut Context<Self>) {
        if let Some(on_disk) = self.conflict.take() {
            self.stamp = Some(on_disk);
            self.save(cx);
            cx.emit(EditorEvent::StatusChanged);
            cx.notify();
        }
    }

    fn input_line(&self) -> (ByteOffset, String) {
        let buffer = self.model.buffer();
        let line = buffer.point_at(self.model.selection().start).line;
        (buffer.line_start(line), buffer.line(line).into_owned())
    }

    fn input_range(&self, range_utf16: &Range<usize>) -> ByteRange {
        let (start, text) = self.input_line();
        let at = |units: usize| ByteOffset(start.get() + utf16_to_byte(&text, units));
        at(range_utf16.start)..at(range_utf16.end)
    }

    fn input_range_utf16(&self, range: &ByteRange) -> Range<usize> {
        let (start, text) = self.input_line();
        let at = |offset: ByteOffset| {
            let column = offset.get().saturating_sub(start.get()).min(text.len());
            utf16_len(text.get(..column).unwrap_or(&text))
        };
        at(range.start)..at(range.end)
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.input_range(&range_utf16);
        adjusted_range.replace(self.input_range_utf16(&range));
        self.model.buffer().text(range).ok()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.model.selection();
        let line_end = {
            let buffer = self.model.buffer();
            buffer.line_end(buffer.point_at(selection.start).line)
        };
        Some(UTF16Selection {
            range: self.input_range_utf16(&(selection.start..selection.end.min(line_end))),
            reversed: self.model.caret() < selection.end,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| self.input_range_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || !self.is_ready() {
            return;
        }
        let range = range_utf16
            .map(|range| self.input_range(&range))
            .or_else(|| self.marked.take());
        self.marked = None;
        let edits = match range {
            Some(range) => self.model.replace_range(range, text),
            None => self.model.insert(text),
        };
        self.edited(&edits, cx);
        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || !self.is_ready() {
            return;
        }
        let range = range_utf16
            .map(|range| self.input_range(&range))
            .or_else(|| self.marked.take())
            .unwrap_or_else(|| self.model.selection());
        let start = range.start;
        let edits = self.model.replace_range(range, new_text);
        let end = ByteOffset(start.get() + new_text.len());
        self.marked = (!new_text.is_empty()).then_some(start..end);
        if let Some(selected) = new_selected_range_utf16 {
            let at = |units: usize| ByteOffset(start.get() + utf16_to_byte(new_text, units));
            self.model.select(at(selected.start)..at(selected.end));
        }
        self.edited(&edits, cx);
        window.invalidate_character_coordinates();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.input_range(&range_utf16);
        let painted = self.painted.as_ref()?;
        let start = self.model.buffer().point_at(range.start);
        let end = self.model.buffer().point_at(range.end);
        let row = painted
            .rows
            .iter()
            .position(|line| *line == start.line.get())?;
        let (display, shaped) = painted.lines.get(row)?;
        let x = |point: TextPoint| {
            painted.text_left - painted.scroll_x
                + shaped.x_for_index(display.to_display(point.column))
        };
        #[expect(clippy::cast_precision_loss, reason = "a row on screen")]
        let top = painted.first_line_top + LINE_HEIGHT * row as f32;
        let right = if end.line == start.line {
            x(end)
        } else {
            x(TextPoint {
                line: start.line,
                column: self.model.buffer().line_len(LineIndex(start.line.get())),
            })
        };
        Some(Bounds::from_corners(
            point(x(start), top),
            point(right, top + LINE_HEIGHT),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_at(point)?;
        Some(self.input_range_utf16(&(offset..offset)).start)
    }
}
