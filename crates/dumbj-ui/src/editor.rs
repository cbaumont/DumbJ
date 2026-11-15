use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dumbj_actions::{Action, MouseStroke};
use dumbj_index::{Index, LibraryTarget, Resolution, Site, SymbolId, UsageTarget};
use dumbj_settings::UiRole;
use dumbj_syntax::{HighlightSpan, Language, SyntaxTree};
use dumbj_text::search::{self, SearchOptions};
use dumbj_text::{Buffer, ByteOffset, ByteRange, LineIndex, Point as TextPoint};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, DispatchPhase, Element,
    ElementId, Entity, EventEmitter, FocusHandle, Font, FontFeatures, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, PaintQuad, Pixels, Point, Render,
    ScrollEvent, ShapedLine, SharedString, Style, Task, TextAlign, TextRun, Window, div, fill,
    font, point, prelude::*, px, relative, size,
};

use crate::assets::EDITOR_FONT;
use crate::editor_model::{DisplayLine, EditorModel, Motion, scroll_to_center, scroll_to_reveal};
use crate::keys::modifiers_from_gpui;
use crate::theme;

const FONT_SIZE: Pixels = px(13.);
const LINE_HEIGHT: Pixels = px(20.);
const GUTTER_PADDING_LEFT: Pixels = px(18.);
const GUTTER_PADDING_RIGHT: Pixels = px(14.);
const TEXT_PADDING_LEFT: Pixels = px(4.);
const SCROLLBAR_WIDTH: Pixels = px(10.);
const SCROLLBAR_MIN_THUMB: Pixels = px(28.);
/// Lines of context kept above and below the caret when scrolling to it.
const REVEAL_MARGIN: f64 = 3.0;
/// Extra lines highlighted around the viewport so small scrolls reuse the cached spans.
const HIGHLIGHT_MARGIN: usize = 60;
/// Longer lines are cut when drawn (degraded mode for minified files).
const MAX_LINE_BYTES: usize = 10_000;
/// Larger files open without syntax highlighting (degraded mode, BRIEF §4.2).
const MAX_HIGHLIGHT_FILE_BYTES: u64 = 20 * 1024 * 1024;

struct Loaded {
    buffer: Buffer,
    syntax: Option<SyntaxTree>,
}

fn load(path: &Path, language: Option<Language>, cancelled: &AtomicBool) -> Result<Loaded, String> {
    let bytes = if dumbj_jars::is_virtual_path(path) {
        dumbj_jars::read_virtual(path)
            .map_err(|error| error.to_string())?
            .into_bytes()
    } else {
        std::fs::read(path).map_err(|error| error.to_string())?
    };
    let degraded = bytes.len() as u64 > MAX_HIGHLIGHT_FILE_BYTES;
    let buffer = Buffer::new(&String::from_utf8_lossy(&bytes));
    let syntax = match language.filter(|_| !degraded) {
        Some(language) => {
            match dumbj_syntax::parse(language, &buffer, || cancelled.load(Ordering::Relaxed)) {
                Ok(tree) => tree,
                Err(error) => {
                    tracing::error!(%error, "syntax highlighting is unavailable");
                    None
                }
            }
        }
        None => None,
    };
    Ok(Loaded { buffer, syntax })
}

enum LoadState {
    Loading,
    Failed(String),
    Ready,
}

#[derive(Clone, Copy)]
enum Reveal {
    Minimal,
    Center,
}

#[derive(Default)]
struct SearchState {
    query: String,
    matches: Arc<Vec<ByteRange>>,
    current: Option<usize>,
    /// Set to stop a search that is still running when the query changes.
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

struct HighlightCache {
    buffer_version: u64,
    lines: Range<usize>,
    spans: Vec<HighlightSpan>,
}

/// What the last paint drew, for mapping mouse positions back to text.
#[derive(Clone)]
struct PaintedLines {
    bounds: Bounds<Pixels>,
    text_left: Pixels,
    first_line_top: Pixels,
    first_line: usize,
    visible_lines: f64,
    scroll_x: Pixels,
    lines: Arc<Vec<(DisplayLine, ShapedLine)>>,
    scrollbar: Option<Bounds<Pixels>>,
}

struct ScrollBenchmark {
    last_frame: Option<Instant>,
    /// Time from one frame's layout to the next: what the user sees.
    frame_times: Vec<Duration>,
    /// Time spent shaping and laying out the visible lines: DumbJ's own share of a frame.
    layout_times: Vec<Duration>,
}

/// Lines scrolled per frame by the benchmark; fractional so every frame moves the text.
const BENCHMARK_STEP: f64 = 1.37;
const BENCHMARK_FRAMES: usize = 600;

#[derive(Clone, Copy)]
enum Drag {
    Selecting,
    /// Dragging the scrollbar thumb, grabbed this far below its top.
    Scrollbar {
        grab: Pixels,
    },
}

/// Emitted for every mouse press in the editor, after the caret has moved there, so the
/// workspace can run a mouse shortcut bound to it.
pub(crate) struct MousePressed(pub(crate) MouseStroke);

/// An implements/overrides marker next to a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GutterIcon {
    pub(crate) offset: ByteOffset,
    pub(crate) symbol: SymbolId,
    pub(crate) overrides: bool,
    pub(crate) overridden: bool,
}

pub(crate) struct GutterIconClicked(pub(crate) GutterIcon);

/// A bookmark marker beside a line number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BookmarkMark {
    /// One-based.
    pub(crate) line: usize,
    pub(crate) mnemonic: Option<char>,
}

pub(crate) struct EditorView {
    path: PathBuf,
    focus: FocusHandle,
    state: LoadState,
    model: EditorModel,
    language: Option<Language>,
    syntax: Option<SyntaxTree>,
    /// The first visible line; fractional while a line is partly scrolled off.
    scroll_top: f64,
    scroll_x: Pixels,
    reveal: Option<Reveal>,
    search: SearchState,
    highlights: Option<HighlightCache>,
    painted: Option<PaintedLines>,
    drag: Option<Drag>,
    pending_offset: Option<ByteOffset>,
    pending_go_to: Option<(usize, Option<usize>)>,
    benchmark: Option<ScrollBenchmark>,
    gutter_icons: Vec<GutterIcon>,
    bookmarks: Vec<BookmarkMark>,
    load_cancel: Arc<AtomicBool>,
    _load: Task<()>,
}

impl EventEmitter<MousePressed> for EditorView {}
impl EventEmitter<GutterIconClicked> for EditorView {}

impl Drop for EditorView {
    fn drop(&mut self) {
        self.load_cancel.store(true, Ordering::Relaxed);
        self.search.cancel.store(true, Ordering::Relaxed);
    }
}

impl EditorView {
    pub(crate) fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let language = Language::from_path(&path);
        let load_cancel = Arc::new(AtomicBool::new(false));
        let (load_path, cancel) = (path.clone(), load_cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load(&load_path, language, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(loaded) => {
                        this.model = EditorModel::new(loaded.buffer);
                        this.syntax = loaded.syntax;
                        this.state = LoadState::Ready;
                        if let Some(offset) = this.pending_offset.take() {
                            this.go_to_offset(offset, cx);
                        }
                        if let Some((line, column)) = this.pending_go_to.take() {
                            this.go_to(line, column, cx);
                        }
                    }
                    Err(message) => this.state = LoadState::Failed(message),
                }
                cx.notify();
            });
        });
        Self {
            path,
            focus: cx.focus_handle(),
            state: LoadState::Loading,
            model: EditorModel::default(),
            language,
            syntax: None,
            scroll_top: 0.0,
            scroll_x: px(0.),
            reveal: None,
            search: SearchState::default(),
            highlights: None,
            painted: None,
            drag: None,
            pending_offset: None,
            pending_go_to: None,
            benchmark: None,
            gutter_icons: Vec::new(),
            bookmarks: Vec::new(),
            load_cancel,
            _load: task,
        }
    }

    pub(crate) fn start_scroll_benchmark(&mut self) {
        self.benchmark = Some(ScrollBenchmark {
            last_frame: None,
            frame_times: Vec::new(),
            layout_times: Vec::new(),
        });
    }

    fn step_scroll_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(benchmark) = self.benchmark.as_mut() else {
            return;
        };
        let now = Instant::now();
        if let Some(last) = benchmark.last_frame.replace(now) {
            benchmark.frame_times.push(now - last);
        }
        if benchmark.frame_times.len() < BENCHMARK_FRAMES {
            self.scroll_top += BENCHMARK_STEP;
            window.request_animation_frame();
            return;
        }
        let mut times = std::mem::take(&mut benchmark.frame_times);
        let mut layouts = std::mem::take(&mut benchmark.layout_times);
        times.sort_unstable();
        layouts.sort_unstable();
        let at = |sorted: &[Duration], p: usize| {
            sorted
                .get(sorted.len() * p / 100)
                .copied()
                .unwrap_or_default()
        };
        let percentile = |p: usize| at(&times, p);
        let total: Duration = times.iter().sum();
        #[expect(clippy::cast_precision_loss, reason = "a few hundred frames")]
        let fps = times.len() as f64 / total.as_secs_f64();
        tracing::info!(
            frames = times.len(),
            lines = self.model.buffer().line_count(),
            fps = format!("{fps:.1}"),
            p50 = ?percentile(50),
            p95 = ?percentile(95),
            p99 = ?percentile(99),
            max = ?times.last().copied().unwrap_or_default(),
            layout_p50 = ?at(&layouts, 50),
            layout_p99 = ?at(&layouts, 99),
            layout_max = ?layouts.last().copied().unwrap_or_default(),
            "scroll benchmark"
        );
        self.benchmark = None;
        cx.quit();
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(crate) fn language(&self) -> Option<Language> {
        self.language
    }

    /// The caret position as shown in the status bar: one-based line and character column.
    pub(crate) fn caret_line_column(&self) -> (usize, usize) {
        let point = self.model.caret_point();
        (
            point.line.display_number(),
            self.model.buffer().char_column(point) + 1,
        )
    }

    /// The trimmed text of the caret's line; `None` until the file has loaded.
    pub(crate) fn caret_line_text(&self) -> Option<String> {
        let buffer = self.model.buffer();
        matches!(self.state, LoadState::Ready).then(|| {
            let line = buffer.line(self.model.caret_point().line);
            line.trim().chars().take(200).collect()
        })
    }

    pub(crate) fn selection_for_search(&self) -> Option<String> {
        let selected = self.model.selected_text();
        (!selected.is_empty() && !selected.contains('\n')).then_some(selected)
    }

    pub(crate) fn search_query(&self) -> &str {
        &self.search.query
    }

    /// The one-based index of the current match and the number of matches.
    pub(crate) fn search_status(&self) -> (Option<usize>, usize) {
        (
            self.search.current.map(|index| index + 1),
            self.search.matches.len(),
        )
    }

    fn page_lines(&self) -> usize {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a viewport holds a small positive number of lines"
        )]
        let visible = self
            .painted
            .as_ref()
            .map_or(30.0, |painted| painted.visible_lines) as usize;
        visible.saturating_sub(1).max(1)
    }

    /// Runs an editor or text action. Returns `false` if the editor has no use for it.
    pub(crate) fn handle_action(&mut self, action: Action, cx: &mut Context<Self>) -> bool {
        let page = self.page_lines();
        let motion = |motion, extend| Some((motion, extend));
        let movement = match action {
            Action::MoveLeft => motion(Motion::Left, false),
            Action::MoveRight => motion(Motion::Right, false),
            Action::MoveUp => motion(Motion::Up, false),
            Action::MoveDown => motion(Motion::Down, false),
            Action::MoveLineStart => motion(Motion::LineStart, false),
            Action::MoveLineEnd => motion(Motion::LineEnd, false),
            Action::MoveTextStart => motion(Motion::TextStart, false),
            Action::MoveTextEnd => motion(Motion::TextEnd, false),
            Action::MovePageUp => motion(Motion::PageUp(page), false),
            Action::MovePageDown => motion(Motion::PageDown(page), false),
            Action::MoveWordLeft => motion(Motion::WordLeft, false),
            Action::MoveWordRight => motion(Motion::WordRight, false),
            Action::SelectLeft => motion(Motion::Left, true),
            Action::SelectRight => motion(Motion::Right, true),
            Action::SelectUp => motion(Motion::Up, true),
            Action::SelectDown => motion(Motion::Down, true),
            Action::SelectLineStart => motion(Motion::LineStart, true),
            Action::SelectLineEnd => motion(Motion::LineEnd, true),
            Action::SelectTextStart => motion(Motion::TextStart, true),
            Action::SelectTextEnd => motion(Motion::TextEnd, true),
            Action::SelectPageUp => motion(Motion::PageUp(page), true),
            Action::SelectPageDown => motion(Motion::PageDown(page), true),
            Action::SelectWordLeft => motion(Motion::WordLeft, true),
            Action::SelectWordRight => motion(Motion::WordRight, true),
            _ => None,
        };
        if let Some((motion, extend)) = movement {
            self.model.apply_motion(motion, extend);
            self.reveal = Some(Reveal::Minimal);
            cx.notify();
            return true;
        }
        match action {
            Action::SelectAll => self.model.select_all(),
            Action::ExtendSelection => {
                self.model.extend_selection();
            }
            Action::Copy => {
                let selected = self.model.selected_text();
                if !selected.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(selected));
                }
            }
            Action::Escape => {
                let caret = self.model.caret();
                self.model.move_to(caret, false);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(crate) fn set_gutter_icons(&mut self, icons: Vec<GutterIcon>, cx: &mut Context<Self>) {
        self.gutter_icons = icons;
        cx.notify();
    }

    pub(crate) fn set_bookmarks(&mut self, marks: Vec<BookmarkMark>, cx: &mut Context<Self>) {
        self.bookmarks = marks;
        cx.notify();
    }

    fn gutter_icon_on_line(&self, line: usize) -> Option<GutterIcon> {
        let buffer = self.model.buffer();
        self.gutter_icons
            .iter()
            .find(|icon| buffer.point_at(icon.offset).line.get() == line)
            .copied()
    }

    pub(crate) fn caret_offset(&self) -> ByteOffset {
        self.model.caret()
    }

    /// Moves the caret to `offset` and centres it. If the file is still loading, this
    /// happens as soon as it has loaded.
    pub(crate) fn go_to_offset(&mut self, offset: ByteOffset, cx: &mut Context<Self>) {
        if matches!(self.state, LoadState::Loading) {
            self.pending_offset = Some(offset);
            return;
        }
        self.model.move_to(offset, false);
        self.reveal = Some(Reveal::Center);
        cx.notify();
    }

    /// Resolves the identifier at the caret against `index` (dumb Go to Declaration).
    /// `None` when the file is not indexed, not parsed, or the caret is not on a name.
    pub(crate) fn resolve_at_caret(&self, index: &Index) -> Option<Resolution> {
        let tree = self.syntax.as_ref()?;
        let buffer = self.model.buffer();
        let file = index.file_by_path(&self.path)?;
        dumbj_index::resolve(
            Site {
                index,
                file,
                tree,
                buffer,
            },
            self.model.caret(),
        )
    }

    pub(crate) fn library_targets_at_caret(&self, index: &Index) -> Vec<LibraryTarget> {
        let (Some(tree), Some(file)) = (self.syntax.as_ref(), index.file_by_path(&self.path))
        else {
            return Vec::new();
        };
        let site = Site {
            index,
            file,
            tree,
            buffer: self.model.buffer(),
        };
        dumbj_index::library_targets(site, self.model.caret())
    }

    pub(crate) fn usage_target_at_caret(&self, index: &Index) -> Option<UsageTarget> {
        let tree = self.syntax.as_ref()?;
        let buffer = self.model.buffer();
        let file = index.file_by_path(&self.path)?;
        dumbj_index::usage_target_at(
            Site {
                index,
                file,
                tree,
                buffer,
            },
            self.model.caret(),
        )
    }

    pub(crate) fn word_at_caret(&self) -> Option<String> {
        let buffer = self.model.buffer();
        buffer.text(buffer.word_range_at(self.model.caret())?).ok()
    }

    pub(crate) fn go_to(&mut self, line: usize, column: Option<usize>, cx: &mut Context<Self>) {
        if matches!(self.state, LoadState::Loading) {
            self.pending_go_to = Some((line, column));
            return;
        }
        self.model.go_to(line, column);
        self.reveal = Some(Reveal::Center);
        cx.notify();
    }

    /// Starts searching for `query` in the background and highlights the matches. The match
    /// at or after the caret becomes current once the search finishes.
    pub(crate) fn set_search_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if query == self.search.query {
            return;
        }
        self.search.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.search = SearchState {
            query: query.to_owned(),
            cancel: cancel.clone(),
            ..SearchState::default()
        };
        cx.notify();
        if query.is_empty() {
            return;
        }
        // Cloning a rope shares its storage, so the search works on a cheap snapshot.
        let (buffer, needle) = (self.model.buffer().clone(), query.to_owned());
        let from = self.model.selection().start;
        self.search.task = Some(cx.spawn(async move |this, cx| {
            let flag = cancel.clone();
            let matches = cx
                .background_spawn(async move {
                    // IDEA's default: case-insensitive unless the query has an upper-case letter.
                    let case_sensitive = needle.chars().any(char::is_uppercase);
                    let options = SearchOptions { case_sensitive };
                    search::find_all(&buffer, &needle, options, || flag.load(Ordering::Relaxed))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                this.search.current = search::next_match(&matches, from);
                this.search.matches = Arc::new(matches);
                this.show_current_match();
                cx.notify();
            });
        }));
    }

    fn show_current_match(&mut self) {
        let current = self
            .search
            .current
            .and_then(|index| self.search.matches.get(index));
        if let Some(found) = current.cloned() {
            self.model.select(found);
            self.reveal = Some(Reveal::Center);
        }
    }

    pub(crate) fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) {
        let selection = self.model.selection();
        let matches = &self.search.matches;
        self.search.current = if forward {
            // Start after the selection's first byte so the current match is skipped.
            search::next_match(matches, ByteOffset(selection.start.get() + 1))
        } else {
            search::previous_match(matches, selection.start)
        };
        self.show_current_match();
        cx.notify();
    }

    /// The buffer offset under a window position, clamped to the nearest painted line.
    fn offset_at(&self, position: Point<Pixels>) -> Option<ByteOffset> {
        let painted = self.painted.as_ref()?;
        let rows = ((position.y - painted.first_line_top) / LINE_HEIGHT)
            .floor()
            .max(0.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rows is a small non-negative number of lines"
        )]
        let row = (rows as usize).min(painted.lines.len().checked_sub(1)?);
        let (display, shaped) = painted.lines.get(row)?;
        let x = position.x - painted.text_left + painted.scroll_x;
        let column = display.to_buffer(shaped.closest_index_for_x(x.max(px(0.))));
        let line = LineIndex(painted.first_line + row);
        Some(self.model.buffer().offset_at(TextPoint { line, column }))
    }

    fn max_scroll_top(&self) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
        let lines = self.model.buffer().line_count() as f64;
        (lines - 1.0).max(0.0)
    }

    fn scroll_by(&mut self, lines: f64, x: Pixels, cx: &mut Context<Self>) {
        self.scroll_top = (self.scroll_top + lines).clamp(0.0, self.max_scroll_top());
        self.scroll_x = (self.scroll_x + x).max(px(0.));
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stroke = MouseStroke {
            modifiers: modifiers_from_gpui(event.modifiers),
            button: match event.button {
                MouseButton::Left => 1,
                MouseButton::Middle => 2,
                MouseButton::Right => 3,
                MouseButton::Navigate(NavigationDirection::Back) => 4,
                MouseButton::Navigate(NavigationDirection::Forward) => 5,
            },
            clicks: u8::try_from(event.click_count.min(2)).unwrap_or(1),
        };
        if event.button != MouseButton::Left {
            cx.emit(MousePressed(stroke));
            return;
        }
        window.focus(&self.focus, cx);
        if let Some(scrollbar) = self.painted.as_ref().and_then(|painted| painted.scrollbar)
            && event.position.x >= scrollbar.left()
        {
            let on_thumb = scrollbar.contains(&event.position);
            let grab = if on_thumb {
                event.position.y - scrollbar.top()
            } else {
                scrollbar.size.height / 2.
            };
            self.drag = Some(Drag::Scrollbar { grab });
            self.drag_scrollbar(event.position, grab, cx);
            return;
        }
        let Some(offset) = self.offset_at(event.position) else {
            return;
        };
        let in_icon_column = self
            .painted
            .as_ref()
            .is_some_and(|painted| event.position.x < painted.bounds.left() + GUTTER_PADDING_LEFT);
        if in_icon_column {
            let line = self.model.buffer().point_at(offset).line.get();
            if let Some(icon) = self.gutter_icon_on_line(line) {
                cx.emit(GutterIconClicked(icon));
                return;
            }
        }
        match event.click_count {
            1 => self.model.move_to(offset, event.modifiers.shift),
            2 => {
                self.model.select_word_at(offset);
            }
            _ => self
                .model
                .select_line(self.model.buffer().point_at(offset).line),
        }
        self.drag = (event.click_count == 1).then_some(Drag::Selecting);
        cx.notify();
        // The caret is already under the pointer, so a bound mouse shortcut such as
        // Ctrl+click acts on what was clicked.
        cx.emit(MousePressed(stroke));
    }

    fn drag_scrollbar(&mut self, position: Point<Pixels>, grab: Pixels, cx: &mut Context<Self>) {
        let Some(painted) = self.painted.as_ref() else {
            return;
        };
        let Some(thumb) = painted.scrollbar else {
            return;
        };
        let track = painted.bounds.size.height - thumb.size.height;
        if track > px(0.) {
            let fraction = ((position.y - painted.bounds.top() - grab) / track).clamp(0.0, 1.0);
            self.scroll_top = f64::from(fraction) * self.max_scroll_top();
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        match self.drag {
            Some(Drag::Selecting) => {
                if let Some(offset) = self.offset_at(event.position) {
                    self.model.move_to(offset, true);
                    self.reveal = Some(Reveal::Minimal);
                    cx.notify();
                }
            }
            Some(Drag::Scrollbar { grab }) => self.drag_scrollbar(event.position, grab, cx),
            None => {}
        }
    }

    fn highlights_for(&mut self, lines: Range<usize>) -> &[HighlightSpan] {
        let buffer = self.model.buffer();
        let version = buffer.version();
        let cached = self.highlights.as_ref().is_some_and(|cache| {
            cache.buffer_version == version
                && cache.lines.start <= lines.start
                && cache.lines.end >= lines.end
        });
        if !cached {
            let start = lines.start.saturating_sub(HIGHLIGHT_MARGIN);
            let end = (lines.end + HIGHLIGHT_MARGIN).min(buffer.line_count());
            let range = buffer.line_start(LineIndex(start))
                ..buffer.line_end(LineIndex(end.saturating_sub(1)));
            let spans = match self
                .syntax
                .as_ref()
                .filter(|tree| tree.buffer_version() == version)
            {
                Some(tree) => tree.highlights(buffer, range).unwrap_or_else(|error| {
                    tracing::error!(%error, "highlighting failed");
                    Vec::new()
                }),
                None => Vec::new(),
            };
            self.highlights = Some(HighlightCache {
                buffer_version: version,
                lines: start..end,
                spans,
            });
        }
        self.highlights
            .as_ref()
            .map_or(&[], |cache| cache.spans.as_slice())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the visible lines; splitting it would thread a dozen locals through helpers"
    )]
    fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EditorLayout {
        let text_color = theme::ui(cx, UiRole::Text);
        // Ligatures are off, as in IDEA's default; a setting comes with the settings UI.
        let editor_font = Font {
            features: FontFeatures::disable_ligatures(),
            ..font(EDITOR_FONT)
        };
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: editor_font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shape = |window: &mut Window, text: String, runs: &[TextRun]| {
            window
                .text_system()
                .shape_line(SharedString::from(text), FONT_SIZE, runs, None)
        };

        let line_count = self.model.buffer().line_count();
        let digits = line_count.to_string().len().max(2);
        let digit_width = shape(window, "0".to_owned(), &[run(1, text_color)]).width;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a line number has at most 20 digits"
        )]
        let gutter_width = GUTTER_PADDING_LEFT + digit_width * digits as f32 + GUTTER_PADDING_RIGHT;
        let text_left = bounds.left() + gutter_width + TEXT_PADDING_LEFT;

        let layout_started = Instant::now();
        self.step_scroll_benchmark(window, cx);
        let visible_lines = f64::from(bounds.size.height / LINE_HEIGHT);
        let caret_point = self.model.caret_point();
        match self.reveal.take() {
            Some(Reveal::Minimal) => {
                self.scroll_top = scroll_to_reveal(
                    self.scroll_top,
                    visible_lines,
                    caret_point.line.get(),
                    REVEAL_MARGIN,
                    line_count,
                );
            }
            Some(Reveal::Center) => {
                self.scroll_top =
                    scroll_to_center(visible_lines, caret_point.line.get(), line_count);
            }
            None => {}
        }
        self.scroll_top = self.scroll_top.clamp(0.0, self.max_scroll_top());

        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "scroll_top is clamped to 0..line_count"
        )]
        let first_line = self.scroll_top.floor() as usize;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the fraction is within 0..1"
        )]
        let first_line_top = bounds.top() - LINE_HEIGHT * self.scroll_top.fract() as f32;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a viewport holds a small positive number of lines"
        )]
        let last_line = (first_line + visible_lines.ceil() as usize + 1).min(line_count);

        // Collect per-line highlight runs before shaping, since both need `self`.
        let spans = self.highlights_for(first_line..last_line).to_vec();
        let buffer = self.model.buffer();
        let mut span_index = spans
            .partition_point(|span| span.range.end <= buffer.line_start(LineIndex(first_line)));
        let selection = self.model.selection();
        let has_selection = self.model.has_selection();
        let focused = self.focus.is_focused(window);
        let selection_color = theme::ui(cx, UiRole::EditorSelection);
        let match_color = theme::ui(cx, UiRole::EditorSearchMatch);
        let current_match_color = theme::ui(cx, UiRole::EditorSearchMatchCurrent);
        let number_color = theme::ui(cx, UiRole::EditorLineNumber);
        let active_number_color = theme::ui(cx, UiRole::EditorLineNumberActive);

        let matches = self.search.matches.clone();
        let visible_start = buffer.line_start(LineIndex(first_line));
        let mut match_index = matches.partition_point(|found| found.end <= visible_start);

        let mut backgrounds = Vec::new();
        let mut lines = Vec::with_capacity(last_line - first_line);
        let mut numbers = Vec::with_capacity(last_line - first_line);
        let mut caret = None;
        let mut max_width = px(0.);
        for line in first_line..last_line {
            let line_index = LineIndex(line);
            #[expect(
                clippy::cast_precision_loss,
                reason = "at most a few hundred visible lines"
            )]
            let top = first_line_top + LINE_HEIGHT * (line - first_line) as f32;
            let (line_start, line_end) =
                (buffer.line_start(line_index), buffer.line_end(line_index));
            let display = DisplayLine::new(&buffer.line(line_index), MAX_LINE_BYTES);
            let column = |offset: ByteOffset| {
                display.to_display(
                    offset.get().clamp(line_start.get(), line_end.get()) - line_start.get(),
                )
            };

            let mut runs = Vec::new();
            let mut position = 0;
            while let Some(span) = spans
                .get(span_index)
                .filter(|span| span.range.start < line_end)
            {
                let (start, end) = (column(span.range.start), column(span.range.end));
                if start > position {
                    runs.push(run(start - position, text_color));
                }
                if end > start.max(position) {
                    runs.push(run(end - start.max(position), theme::syntax(cx, span.kind)));
                    position = end;
                }
                if span.range.end > line_end {
                    break; // The span continues on the next line.
                }
                span_index += 1;
            }
            if display.text.len() > position {
                runs.push(run(display.text.len() - position, text_color));
            }
            let shaped = shape(window, display.text.clone(), &runs);
            max_width = max_width.max(shaped.width);
            let x =
                |offset: ByteOffset| text_left - self.scroll_x + shaped.x_for_index(column(offset));
            let band = |from: Pixels, to: Pixels, color: Hsla| {
                fill(
                    Bounds::from_corners(point(from, top), point(to, top + LINE_HEIGHT)),
                    color,
                )
            };

            if line == caret_point.line.get() && !has_selection {
                let current_line = theme::ui(cx, UiRole::EditorCurrentLine);
                backgrounds.push(band(bounds.left(), bounds.right(), current_line));
            }
            while let Some(found) = matches
                .get(match_index)
                .filter(|found| found.start < line_end)
            {
                let is_current = self.search.current == Some(match_index);
                let color = if is_current {
                    current_match_color
                } else {
                    match_color
                };
                backgrounds.push(band(x(found.start), x(found.end), color));
                match_index += 1;
            }
            if has_selection && selection.start <= line_end && selection.end > line_start {
                // A selection that runs past the line end also covers the line break.
                let tail = if selection.end > line_end {
                    digit_width
                } else {
                    px(0.)
                };
                let from = x(selection.start.max(line_start));
                backgrounds.push(band(
                    from,
                    x(selection.end.min(line_end)) + tail,
                    selection_color,
                ));
            }
            if line == caret_point.line.get() && focused {
                let caret_bounds =
                    Bounds::new(point(x(self.model.caret()), top), size(px(2.), LINE_HEIGHT));
                caret = Some(fill(caret_bounds, theme::ui(cx, UiRole::EditorCaret)));
            }

            let number = line_index.display_number().to_string();
            let is_caret_line = line == caret_point.line.get();
            let color = if is_caret_line {
                active_number_color
            } else {
                number_color
            };
            let number = shape(window, number.clone(), &[run(number.len(), color)]);
            let number_x = bounds.left() + gutter_width - GUTTER_PADDING_RIGHT - number.width;
            let bookmark_x = number_x + number.width + px(5.);
            numbers.push((point(number_x, top), number));
            if let Some(mark) = self
                .bookmarks
                .iter()
                .find(|mark| mark.line == line_index.display_number())
            {
                let glyph = mark.mnemonic.map_or("●".to_owned(), String::from);
                let marker_color = theme::ui(cx, UiRole::Warning);
                let glyph = shape(window, glyph.clone(), &[run(glyph.len(), marker_color)]);
                numbers.push((point(bookmark_x, top), glyph));
            }
            if let Some(icon) = self.gutter_icon_on_line(line) {
                let (glyph, role) = match (icon.overrides, icon.overridden) {
                    (true, true) => ("↕", UiRole::Accent),
                    (true, false) => ("↑", UiRole::Info),
                    _ => ("↓", UiRole::Success),
                };
                let glyph = shape(
                    window,
                    glyph.to_owned(),
                    &[run(glyph.len(), theme::ui(cx, role))],
                );
                numbers.push((point(bounds.left() + px(4.), top), glyph));
            }
            lines.push((display, shaped));
        }

        let text_width = bounds.right() - text_left - SCROLLBAR_WIDTH;
        self.scroll_x = self.scroll_x.min((max_width - text_width / 2.).max(px(0.)));

        #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
        let fits = line_count as f64 <= visible_lines;
        let scrollbar = (!fits).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
            let total = line_count as f64 + visible_lines - 1.0;
            #[expect(clippy::cast_possible_truncation, reason = "fractions within 0..1")]
            let (fraction, position) = (
                (visible_lines / total) as f32,
                (self.scroll_top / self.max_scroll_top()) as f32,
            );
            let height = (bounds.size.height * fraction).max(SCROLLBAR_MIN_THUMB);
            let top = bounds.top() + (bounds.size.height - height) * position;
            Bounds::new(
                point(bounds.right() - SCROLLBAR_WIDTH, top),
                size(SCROLLBAR_WIDTH, height),
            )
        });

        if let Some(benchmark) = self.benchmark.as_mut() {
            benchmark.layout_times.push(layout_started.elapsed());
        }
        let lines = Arc::new(lines);
        self.painted = Some(PaintedLines {
            bounds,
            text_left,
            first_line_top,
            first_line,
            visible_lines,
            scroll_x: self.scroll_x,
            lines: lines.clone(),
            scrollbar,
        });
        EditorLayout {
            text_bounds: Bounds::from_corners(
                point(bounds.left() + gutter_width, bounds.top()),
                bounds.bottom_right(),
            ),
            text_origin: point(text_left - self.scroll_x, first_line_top),
            lines,
            numbers,
            backgrounds,
            caret,
            scrollbar: scrollbar.map(|thumb| {
                let color = theme::ui(cx, UiRole::TextFaint).opacity(0.35);
                fill(thumb, color).corner_radii(px(4.))
            }),
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = match &self.state {
            LoadState::Ready => None,
            LoadState::Loading => Some(("Loading…".to_owned(), UiRole::TextFaint)),
            LoadState::Failed(error) => Some((
                format!("Cannot open {}: {error}", self.path.display()),
                UiRole::Error,
            )),
        };
        let content = match message {
            Some((text, role)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::ui(cx, role))
                .child(text)
                .into_any_element(),
            None => EditorElement {
                editor: cx.entity(),
            }
            .into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .size_full()
            .bg(theme::ui(cx, UiRole::Background))
            .cursor(CursorStyle::IBeam)
            .child(content)
    }
}

struct EditorElement {
    editor: Entity<EditorView>,
}

struct EditorLayout {
    text_bounds: Bounds<Pixels>,
    text_origin: Point<Pixels>,
    lines: Arc<Vec<(DisplayLine, ShapedLine)>>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    backgrounds: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    scrollbar: Option<PaintQuad>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = (EditorLayout, Hitbox);

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let layout = self
            .editor
            .update(cx, |editor, cx| editor.layout(bounds, window, cx));
        (layout, hitbox)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        (layout, hitbox): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (origin, number) in &layout.numbers {
                paint_line(number, *origin, window, cx);
            }
            window.with_content_mask(
                Some(ContentMask {
                    bounds: layout.text_bounds,
                }),
                |window| {
                    for quad in layout.backgrounds.drain(..) {
                        window.paint_quad(quad);
                    }
                    let mut origin = layout.text_origin;
                    for (_, line) in layout.lines.iter() {
                        paint_line(line, origin, window, cx);
                        origin.y += LINE_HEIGHT;
                    }
                    if let Some(caret) = layout.caret.take() {
                        window.paint_quad(caret);
                    }
                },
            );
            if let Some(scrollbar) = layout.scrollbar.take() {
                window.paint_quad(scrollbar);
            }
        });

        let editor = self.editor.clone();
        let hovered = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hovered.is_hovered(window) {
                editor.update(cx, |editor, cx| editor.on_mouse_down(event, window, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                editor.update(cx, |editor, cx| editor.on_mouse_move(event, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                editor.update(cx, |editor, _| editor.drag = None);
            }
        });
        let editor = self.editor.clone();
        let hovered = hitbox.clone();
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hovered.is_hovered(window) {
                let delta = event.delta.pixel_delta(LINE_HEIGHT);
                let (dx, dy) = if event.modifiers.shift && delta.x == px(0.) {
                    (delta.y, px(0.))
                } else {
                    (delta.x, delta.y)
                };
                editor.update(cx, |editor, cx| {
                    editor.scroll_by(f64::from(-dy / LINE_HEIGHT), -dx, cx);
                });
            }
        });
    }
}

fn paint_line(line: &ShapedLine, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    if let Err(error) = line.paint(origin, LINE_HEIGHT, TextAlign::Left, None, window, cx) {
        tracing::warn!(%error, "failed to paint a line");
    }
}
