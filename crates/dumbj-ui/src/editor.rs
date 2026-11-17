use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dumbj_actions::{Action, MouseStroke};
use dumbj_index::{Index, LibraryTarget, Resolution, Site, SymbolId, UsageHighlight, UsageTarget};
use dumbj_settings::UiRole;
use dumbj_syntax::{HighlightSpan, Language, SyntaxTree};
use dumbj_text::disk::FileStamp;
use dumbj_text::search::{self, SearchOptions};
use dumbj_text::{Buffer, ByteOffset, ByteRange, LineIndex, Point as TextPoint};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, EventEmitter, FocusHandle, Font, FontFeatures,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, PaintQuad,
    Pixels, Point, Render, ScrollEvent, ShapedLine, SharedString, Style, Task, TextAlign, TextRun,
    Window, div, fill, font, point, prelude::*, px, relative, size,
};

use crate::assets::EDITOR_FONT;
use crate::editor_model::{
    DisplayLine, EditorModel, FoldKind, FoldRegion, Format, Motion, scroll_to_center,
    scroll_to_reveal,
};
use crate::keys::modifiers_from_gpui;
use crate::theme;

mod annotate;
mod blocks;
mod changes;
mod diagnostics;
mod diff_marks;
mod editing;
mod semantic;

use blocks::Slot;
pub(crate) use blocks::{Block, BlockRow, BlockTone};

pub(crate) use diff_marks::DiffMarks;

pub(crate) use editing::EditorEvent;

const FONT_SIZE: Pixels = px(13.);
const LINE_HEIGHT: Pixels = px(20.);
const GUTTER_PADDING_LEFT: Pixels = px(18.);
const MAX_STICKY_LINES: usize = 5;
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
    folds: Vec<FoldRegion>,
    format: Format,
    /// `None` for files that cannot be saved: library sources and files that are not UTF-8.
    stamp: Option<FileStamp>,
}

fn fold_regions(tree: &SyntaxTree, buffer: &Buffer) -> Vec<FoldRegion> {
    tree.folds(buffer)
        .into_iter()
        .map(|fold| FoldRegion {
            header: fold.header.get(),
            start: fold.start.get(),
            end: fold.end.get(),
            kind: fold.kind,
        })
        .collect()
}

fn load(
    path: &Path,
    revision: Option<&(dumbj_vcs::Repository, String)>,
    language: Option<Language>,
    cancelled: &AtomicBool,
) -> Result<Loaded, String> {
    let (bytes, stamp) = if let Some((repository, revision)) = revision {
        let text = repository
            .text_at(revision, path)
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        (text.into_bytes(), None)
    } else if let Some((workdir, commit, relative)) = dumbj_vcs::split_revision_path(path) {
        let text = dumbj_vcs::Repository::discover(&workdir)
            .and_then(|repository| match repository {
                Some(repository) => repository.text_at(&commit, &relative),
                None => Ok(None),
            })
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        (text.into_bytes(), None)
    } else if dumbj_jars::is_virtual_path(path) {
        let text = dumbj_jars::read_virtual(path).map_err(|error| error.to_string())?;
        (text.into_bytes(), None)
    } else {
        let stamp = FileStamp::of(path).ok();
        (
            std::fs::read(path).map_err(|error| error.to_string())?,
            stamp,
        )
    };
    let degraded = bytes.len() as u64 > MAX_HIGHLIGHT_FILE_BYTES;
    let text = String::from_utf8_lossy(&bytes);
    // A lossy decoding cannot be written back without damaging the file.
    let stamp = stamp.filter(|_| matches!(text, std::borrow::Cow::Borrowed(_)));
    let buffer = Buffer::new(&text);
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
    let folds = syntax
        .as_ref()
        .map_or_else(Vec::new, |tree| fold_regions(tree, &buffer));
    Ok(Loaded {
        format: Format::detect(&buffer),
        buffer,
        syntax,
        folds,
        stamp,
    })
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
    /// The buffer line shown in each painted row.
    rows: Vec<usize>,
    /// For each painted row that belongs to a block, the block's id.
    blocks: Vec<Option<usize>>,
    gutter_right: Pixels,
    visible_lines: f64,
    scroll_x: Pixels,
    /// How many characters fit between the gutter and the scrollbar.
    columns: usize,
    lines: Arc<Vec<(DisplayLine, ShapedLine)>>,
    scrollbar: Option<Bounds<Pixels>>,
    sticky_lines: Vec<usize>,
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

/// The right button was pressed at this window position; the caret is already there.
pub(crate) struct ContextMenuRequested(pub(crate) Point<Pixels>);

/// An implements/overrides marker next to a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GutterIcon {
    pub(crate) offset: ByteOffset,
    pub(crate) symbol: SymbolId,
    pub(crate) overrides: bool,
    pub(crate) overridden: bool,
}

pub(crate) struct GutterIconClicked(pub(crate) GutterIcon);

/// The `+` the gutter offers on the line under the pointer was clicked; the caret is on
/// that line.
pub(crate) struct LineActionClicked;

#[derive(Default)]
struct LineAction {
    offered: bool,
    hover: Option<usize>,
}

/// A click on the block with this id.
pub(crate) struct BlockClicked(pub(crate) usize);

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
    line_action: LineAction,
    index: Option<Arc<Index>>,
    blocks: Vec<Block>,
    usages: Arc<Vec<UsageHighlight>>,
    usages_at: Option<ByteOffset>,
    usages_task: Option<Task<()>>,
    bookmarks: Vec<BookmarkMark>,
    changes: changes::Changes,
    diff_marks: DiffMarks,
    problems: Arc<Vec<diagnostics::Problem>>,
    /// Colours from the language server, laid over tree-sitter's.
    semantic: Arc<Vec<HighlightSpan>>,
    /// Inlay hints from the language server, sorted by offset.
    inlays: Arc<Vec<(ByteOffset, String)>>,
    note_lines: Vec<(usize, usize)>,
    annotations: Option<Arc<Vec<String>>>,
    read_only: bool,
    /// Text that belongs to no file and is never saved.
    scratch: bool,
    /// What the file looked like on disk when it was last read or written.
    stamp: Option<FileStamp>,
    /// The file's newer state on disk, while it also has unsaved edits here.
    conflict: Option<FileStamp>,
    saving: bool,
    idle_save: Option<Task<()>>,
    /// The text an input method is composing.
    marked: Option<ByteRange>,
    reparse_cancel: Arc<AtomicBool>,
    load_cancel: Arc<AtomicBool>,
    load: Task<()>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl EventEmitter<MousePressed> for EditorView {}
impl EventEmitter<ContextMenuRequested> for EditorView {}
impl EventEmitter<GutterIconClicked> for EditorView {}
impl EventEmitter<LineActionClicked> for EditorView {}
impl EventEmitter<BlockClicked> for EditorView {}

impl Drop for EditorView {
    fn drop(&mut self) {
        self.load_cancel.store(true, Ordering::Relaxed);
        self.reparse_cancel.store(true, Ordering::Relaxed);
        self.search.cancel.store(true, Ordering::Relaxed);
    }
}

impl EditorView {
    pub(crate) fn open(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self::open_source(path, None, cx)
    }

    /// Opens `path` as it is in `revision`, read-only. A file that does not exist there
    /// opens empty.
    pub(crate) fn open_revision(
        path: PathBuf,
        repository: dumbj_vcs::Repository,
        revision: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::open_source(path, Some((repository, revision)), cx)
    }

    /// An editor for text that belongs to no file (a comment being written): it starts
    /// empty and editable and is never saved. `name` only picks the language.
    pub(crate) fn scratch(name: &str, cx: &mut Context<Self>) -> Self {
        let mut editor = Self::open_source(PathBuf::from(name), None, cx);
        editor.load_cancel.store(true, Ordering::Relaxed);
        editor.load = Task::ready(());
        editor.scratch = true;
        editor.set_scratch_text("", cx);
        editor
    }

    /// Replaces the text of a scratch editor and puts the caret at its end.
    pub(crate) fn set_scratch_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.scratch {
            return;
        }
        let buffer = Buffer::new(text);
        let end = buffer.len();
        self.syntax = self.language.and_then(|language| {
            dumbj_syntax::parse(language, &buffer, || false)
                .ok()
                .flatten()
        });
        self.model = EditorModel::new(buffer);
        self.model.move_to(end, false);
        self.highlights = None;
        self.read_only = false;
        self.state = LoadState::Ready;
        self.scroll_top = 0.0;
        cx.notify();
    }

    pub(crate) fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, LoadState::Ready)
    }

    /// The buffer's version once the file is loaded.
    pub(crate) fn document_version(&self) -> Option<u64> {
        self.is_ready().then(|| self.model.buffer().version())
    }

    /// The caret as a language server counts it, once the file is loaded.
    pub(crate) fn lsp_caret(&self) -> Option<lsp_types::Position> {
        self.is_ready()
            .then(|| dumbj_lsp::to_lsp(self.model.buffer(), self.caret_offset()))
    }

    pub(crate) fn text(&self) -> String {
        self.model.buffer().to_string()
    }

    fn open_source(
        path: PathBuf,
        revision: Option<(dumbj_vcs::Repository, String)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = Language::from_path(&path);
        let load_cancel = Arc::new(AtomicBool::new(false));
        let (load_path, cancel) = (path.clone(), load_cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(
                    async move { load(&load_path, revision.as_ref(), language, &cancel) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(loaded) => {
                        this.model = EditorModel::new(loaded.buffer);
                        this.model.set_format(loaded.format);
                        this.read_only = loaded.stamp.is_none();
                        this.stamp = loaded.stamp;
                        this.model.folds.set_regions(loaded.folds);
                        this.model.folds.collapse_all(Some(FoldKind::Imports));
                        this.syntax = loaded.syntax;
                        this.state = LoadState::Ready;
                        this.changes_loaded(cx);
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
            line_action: LineAction::default(),
            index: None,
            blocks: Vec::new(),
            usages: Arc::new(Vec::new()),
            usages_at: None,
            usages_task: None,
            bookmarks: Vec::new(),
            changes: changes::Changes::default(),
            diff_marks: DiffMarks::default(),
            problems: Arc::new(Vec::new()),
            semantic: Arc::new(Vec::new()),
            inlays: Arc::new(Vec::new()),
            note_lines: Vec::new(),
            annotations: None,
            read_only: true,
            scratch: false,
            stamp: None,
            conflict: None,
            saving: false,
            idle_save: None,
            marked: None,
            reparse_cancel: Arc::default(),
            load_cancel,
            load: task,
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
        self.is_ready().then(|| {
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
        if self.handle_edit_action(action, cx) {
            return true;
        }
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
                let syntax = &self.syntax;
                self.model
                    .extend_selection(|range| syntax.as_ref()?.enclosing_range(range));
            }
            Action::ShrinkSelection => {
                self.model.shrink_selection();
            }
            Action::CollapseRegion => {
                let line = self.model.caret_point().line.get();
                if self.model.folds.collapse_at(line) {
                    let start = self.model.folds.row_of(line);
                    let header = self.model.folds.line_of(start);
                    let offset = self.model.buffer().line_end(LineIndex(header));
                    self.model.move_to(offset, false);
                }
            }
            Action::ExpandRegion => {
                let line = self.model.caret_point().line.get();
                self.model.folds.expand_at(line);
            }
            Action::CollapseAllRegions => self.model.folds.collapse_all(None),
            Action::ExpandAllRegions => self.model.folds.expand_all(),
            Action::NextChange => self.go_to_change(true, cx),
            Action::PreviousChange => self.go_to_change(false, cx),
            Action::RollbackChangedLines => {
                if !self.read_only {
                    self.rollback_change(cx);
                }
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

    pub(crate) fn set_index(&mut self, index: Arc<Index>, cx: &mut Context<Self>) {
        self.index = Some(index);
        self.usages_at = None;
        cx.notify();
    }

    fn refresh_usages(&mut self, cx: &mut Context<Self>) {
        let caret = self.model.caret();
        if self.usages_at == Some(caret) {
            return;
        }
        self.usages_at = Some(caret);
        let (Some(index), Some(tree)) = (self.index.clone(), self.syntax.clone()) else {
            return;
        };
        let Some(file) = index.file_by_path(&self.path) else {
            return;
        };
        let buffer = self.model.buffer().clone();
        self.usages_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let found = cx
                .background_spawn(async move {
                    let site = Site {
                        index: &index,
                        file,
                        tree: &tree,
                        buffer: &buffer,
                    };
                    let mut found =
                        dumbj_index::usage_highlights(site, caret, &AtomicBool::new(false));
                    found.sort_by_key(|usage| usage.range.start);
                    found
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.usages = Arc::new(found);
                cx.notify();
            });
        }));
    }

    pub(crate) fn set_gutter_icons(&mut self, icons: Vec<GutterIcon>, cx: &mut Context<Self>) {
        self.gutter_icons = icons;
        cx.notify();
    }

    pub(crate) fn set_bookmarks(&mut self, marks: Vec<BookmarkMark>, cx: &mut Context<Self>) {
        self.bookmarks = marks;
        cx.notify();
    }

    /// Offers a `+` in the gutter of the line under the pointer.
    pub(crate) fn set_line_action(&mut self, offered: bool, cx: &mut Context<Self>) {
        self.line_action.offered = offered;
        cx.notify();
    }

    fn gutter_icon_on_line(&self, line: usize) -> Option<GutterIcon> {
        let buffer = self.model.buffer();
        self.gutter_icons
            .iter()
            .find(|icon| buffer.point_at(icon.offset).line.get() == line)
            .copied()
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
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

    fn site<'a>(&'a self, index: &'a Index) -> Option<Site<'a>> {
        Some(Site {
            index,
            file: index.file_by_path(&self.path)?,
            tree: self.syntax.as_ref()?,
            buffer: self.model.buffer(),
        })
    }

    /// Resolves the identifier at the caret against `index` (dumb Go to Declaration).
    /// `None` when the file is not indexed, not parsed, or the caret is not on a name.
    pub(crate) fn resolve_at_caret(&self, index: &Index) -> Option<Resolution> {
        dumbj_index::resolve(self.site(index)?, self.model.caret())
    }

    pub(crate) fn library_targets_at_caret(&self, index: &Index) -> Vec<LibraryTarget> {
        self.site(index)
            .map_or_default(|site| dumbj_index::library_targets(site, self.model.caret()))
    }

    pub(crate) fn usage_target_at_caret(&self, index: &Index) -> Option<UsageTarget> {
        dumbj_index::usage_target_at(self.site(index)?, self.model.caret())
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
        let line = LineIndex(*painted.rows.get(row)?);
        if painted.blocks.get(row).is_some_and(Option::is_some) {
            return Some(self.model.buffer().line_start(line));
        }
        let (display, shaped) = painted.lines.get(row)?;
        let x = position.x - painted.text_left + painted.scroll_x;
        let column = display.to_buffer(shaped.closest_index_for_x(x.max(px(0.))));
        Some(self.model.buffer().offset_at(TextPoint { line, column }))
    }

    /// How many characters fit on a line, once the editor has been painted.
    pub(crate) fn text_columns(&self) -> Option<usize> {
        self.painted.as_ref().map(|painted| painted.columns)
    }

    /// Where a popup for the caret goes: under the caret, if it is on screen.
    pub(crate) fn caret_popup_position(&self) -> Option<Point<Pixels>> {
        let painted = self.painted.as_ref()?;
        let caret = self.model.caret_point();
        let row = (0..painted.rows.len()).find(|row| {
            painted.rows.get(*row) == Some(&caret.line.get())
                && painted.blocks.get(*row).is_some_and(Option::is_none)
        })?;
        let (display, shaped) = painted.lines.get(row)?;
        let x = painted.text_left - painted.scroll_x
            + shaped.x_for_index(display.to_display(caret.column));
        #[expect(clippy::cast_precision_loss, reason = "a row on screen")]
        let y = painted.first_line_top + LINE_HEIGHT * (row + 1) as f32;
        Some(gpui::point(x, y))
    }

    /// The id of the block painted under a window position.
    fn block_at(&self, position: Point<Pixels>) -> Option<usize> {
        let painted = self.painted.as_ref()?;
        let rows = ((position.y - painted.first_line_top) / LINE_HEIGHT).floor();
        if rows < 0.0 || position.x < painted.gutter_right {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rows is a small non-negative number of lines"
        )]
        let block = painted.blocks.get(rows as usize)?;
        *block
    }

    fn max_scroll_top(&self) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
        let lines = self
            .block_rows()
            .count(self.model.folds.row_count(self.model.buffer().line_count()))
            as f64;
        (lines - 1.0).max(0.0)
    }

    fn scroll_by(&mut self, lines: f64, x: Pixels, cx: &mut Context<Self>) {
        self.scroll_top = (self.scroll_top + lines).clamp(0.0, self.max_scroll_top());
        self.scroll_x = (self.scroll_x + x).max(px(0.));
        cx.notify();
    }

    fn on_right_mouse_down(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let on_block = self.block_at(position).is_some();
        if let Some(offset) = self.offset_at(position).filter(|_| !on_block) {
            let selection = self.model.selection();
            if !(selection.start <= offset && offset <= selection.end) {
                self.model.move_to(offset, false);
            }
        }
        cx.notify();
        cx.emit(ContextMenuRequested(position));
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
        if event.button == MouseButton::Right {
            self.on_right_mouse_down(event.position, window, cx);
            return;
        }
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
        if let Some(painted) = self.painted.as_ref() {
            let rows = ((event.position.y - painted.bounds.top()) / LINE_HEIGHT).floor();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a small non-negative row count"
            )]
            let row = rows.max(0.0) as usize;
            if let Some(&line) = painted.sticky_lines.get(row) {
                let offset = self.model.buffer().line_start(LineIndex(line));
                self.model.move_to(offset, false);
                self.reveal = Some(Reveal::Center);
                cx.notify();
                return;
            }
        }
        if let Some(block) = self.block_at(event.position) {
            cx.emit(BlockClicked(block));
            return;
        }
        let Some(offset) = self.offset_at(event.position) else {
            return;
        };
        let on_fold_marker = self.painted.as_ref().is_some_and(|painted| {
            event.position.x >= painted.gutter_right - GUTTER_PADDING_RIGHT
                && event.position.x < painted.gutter_right
        });
        let clicked_line = self.model.buffer().point_at(offset).line;
        if on_fold_marker && self.model.folds.toggle(clicked_line.get()) {
            cx.notify();
            return;
        }
        let in_icon_column = self
            .painted
            .as_ref()
            .is_some_and(|painted| event.position.x < painted.bounds.left() + GUTTER_PADDING_LEFT);
        if in_icon_column {
            if let Some(icon) = self.gutter_icon_on_line(clicked_line.get()) {
                cx.emit(GutterIconClicked(icon));
                return;
            }
            if self.line_action.offered {
                self.model.move_to(offset, false);
                cx.notify();
                cx.emit(LineActionClicked);
                return;
            }
        }
        match event.click_count {
            1 => self.model.move_to(offset, event.modifiers.shift),
            2 => {
                self.model.select_word_at(offset);
            }
            _ => self.model.select_line(clicked_line),
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

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        if self.line_action.offered {
            let line = self
                .offset_at(event.position)
                .filter(|_| hovered && self.block_at(event.position).is_none())
                .map(|offset| self.model.buffer().point_at(offset).line.get());
            if line != self.line_action.hover {
                self.line_action.hover = line;
                cx.notify();
            }
        }
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
                Some(tree) => tree
                    .highlights(buffer, range.clone())
                    .unwrap_or_else(|error| {
                        tracing::error!(%error, "highlighting failed");
                        Vec::new()
                    }),
                None => Vec::new(),
            };
            let first = self
                .semantic
                .partition_point(|span| span.range.end <= range.start);
            let last = self
                .semantic
                .partition_point(|span| span.range.start < range.end);
            let spans = semantic::overlay(spans, self.semantic.get(first..last).unwrap_or(&[]));
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
        let annotation_chars = self.annotation_width();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a line number has at most 20 digits"
        )]
        let gutter_width = GUTTER_PADDING_LEFT
            + digit_width * (digits + annotation_chars) as f32
            + GUTTER_PADDING_RIGHT;
        let text_left = bounds.left() + gutter_width + TEXT_PADDING_LEFT;

        let layout_started = Instant::now();
        self.step_scroll_benchmark(window, cx);
        let visible_lines = f64::from(bounds.size.height / LINE_HEIGHT);
        let caret_point = self.model.caret_point();
        let block_rows = self.block_rows();
        let row_count = block_rows.count(self.model.folds.row_count(line_count));
        let caret_row = block_rows.visual_of(self.model.folds.row_of(caret_point.line.get()));
        let revealed = self.reveal.is_some();
        match self.reveal.take() {
            Some(Reveal::Minimal) => {
                self.scroll_top = scroll_to_reveal(
                    self.scroll_top,
                    visible_lines,
                    caret_row,
                    REVEAL_MARGIN,
                    row_count,
                );
            }
            Some(Reveal::Center) => {
                self.scroll_top = scroll_to_center(visible_lines, caret_row, row_count);
            }
            None => {}
        }
        let scroll_before = self.scroll_top;
        self.scroll_top = self.scroll_top.clamp(0.0, self.max_scroll_top());
        if (self.scroll_top - scroll_before).abs() > f64::EPSILON || revealed {
            // Tells whoever follows this editor's scrolling (a diff's other side).
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }

        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "scroll_top is clamped to 0..line_count"
        )]
        let first_row = self.scroll_top.floor() as usize;
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
        let last_row = (first_row + visible_lines.ceil() as usize + 1).min(row_count);
        let first_line = self.model.folds.line_of(block_rows.text_row(first_row));
        let last_text_row = block_rows.text_row(last_row.saturating_sub(1));
        let last_line = (self.model.folds.line_of(last_text_row) + 1).min(line_count);

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
        let usages = self.usages.clone();
        let mut usage_index = usages.partition_point(|usage| usage.range.end <= visible_start);
        let problems = self.problems.clone();
        let usage_read_color = theme::ui(cx, UiRole::EditorUsageRead);
        let usage_write_color = theme::ui(cx, UiRole::EditorUsageWrite);

        let mut backgrounds = Vec::new();
        let mut lines = Vec::with_capacity(last_row - first_row);
        let mut numbers = Vec::with_capacity(last_row - first_row);
        let mut placeholders = Vec::new();
        let mut change_marks = Vec::new();
        let mut painted_rows = Vec::with_capacity(last_row - first_row);
        let mut painted_blocks = Vec::with_capacity(last_row - first_row);
        let block_background = theme::ui(cx, UiRole::PanelBackground);
        let mut caret = None;
        let mut max_width = px(0.);
        let folds = &self.model.folds;
        let fold_color = theme::ui(cx, UiRole::TextFaint);
        for visual in first_row..last_row {
            #[expect(
                clippy::cast_precision_loss,
                reason = "at most a few hundred visible lines"
            )]
            let top = first_line_top + LINE_HEIGHT * (visual - first_row) as f32;
            let row = match block_rows.slot(visual) {
                Slot::Text(row) => row,
                Slot::Block { block, row } => {
                    let block = self.blocks.get(block);
                    let content = block.and_then(|block| block.rows.get(row));
                    let (text, tone) = content.map_or(("", BlockTone::Text), |content| {
                        (content.text.as_str(), content.tone)
                    });
                    let role = match tone {
                        BlockTone::Title => UiRole::Accent,
                        BlockTone::Text => UiRole::Text,
                        BlockTone::Faint | BlockTone::Code => UiRole::TextMuted,
                        BlockTone::Suggestion => UiRole::Success,
                    };
                    let display = DisplayLine::new(text, MAX_LINE_BYTES);
                    let shaped = shape(
                        window,
                        display.text.clone(),
                        &[run(display.text.len(), theme::ui(cx, role))],
                    );
                    backgrounds.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + gutter_width, top),
                            point(bounds.right() - SCROLLBAR_WIDTH, top + LINE_HEIGHT),
                        ),
                        block_background,
                    ));
                    lines.push((display, shaped));
                    painted_rows.push(block.map_or(0, |block| block.line));
                    painted_blocks.push(Some(block.map_or(0, |block| block.id)));
                    continue;
                }
            };
            let line = folds.line_of(row);
            painted_rows.push(line);
            painted_blocks.push(None);
            let line_index = LineIndex(line);
            let (line_start, line_end) =
                (buffer.line_start(line_index), buffer.line_end(line_index));
            while spans
                .get(span_index)
                .is_some_and(|span| span.range.end <= line_start)
            {
                span_index += 1;
            }
            while matches
                .get(match_index)
                .is_some_and(|found| found.end <= line_start)
            {
                match_index += 1;
            }
            while usages
                .get(usage_index)
                .is_some_and(|usage| usage.range.end <= line_start)
            {
                usage_index += 1;
            }
            let display = DisplayLine::with_inlays(
                &buffer.line(line_index),
                MAX_LINE_BYTES,
                &self.inlays_in(line_start, line_end),
            );
            let column = |offset: ByteOffset| {
                display.to_display(
                    offset.get().clamp(line_start.get(), line_end.get()) - line_start.get(),
                )
            };

            let (runs, consumed) = highlight_runs(
                &spans[span_index..],
                line_end,
                display.text.len(),
                column,
                |len, kind| run(len, kind.map_or(text_color, |kind| theme::syntax(cx, kind))),
            );
            span_index += consumed;
            let inlay_ranges: Vec<_> = display.inlay_ranges().collect();
            let runs = semantic::mark_inlays(runs, &inlay_ranges, fold_color, block_background);
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
            if let Some(role) = self.diff_marks.role_of(line) {
                let color = theme::ui(cx, role);
                backgrounds.push(band(bounds.left(), bounds.right(), color.opacity(0.13)));
                for word in self.diff_marks.words_in(&(line_start..line_end)) {
                    let (from, to) = (word.start.max(line_start), word.end.min(line_end));
                    backgrounds.push(band(x(from), x(to), color.opacity(0.3)));
                }
            }
            if self.diff_marks.gaps.contains(&line) {
                backgrounds.push(fill(
                    Bounds::new(
                        point(bounds.left(), top - px(0.5)),
                        size(bounds.size.width, px(1.5)),
                    ),
                    theme::ui(cx, UiRole::TextFaint),
                ));
            }
            while let Some(usage) = usages
                .get(usage_index)
                .filter(|usage| usage.range.start < line_end)
            {
                let color = if usage.write {
                    usage_write_color
                } else {
                    usage_read_color
                };
                backgrounds.push(band(x(usage.range.start), x(usage.range.end), color));
                usage_index += 1;
            }
            for problem in problems
                .iter()
                .filter(|p| p.range.start <= line_end && p.range.end >= line_start)
            {
                let from = x(problem.range.start.max(line_start));
                let to = x(problem.range.end.min(line_end)).max(from + px(6.));
                backgrounds.push(fill(
                    Bounds::new(
                        point(from, top + LINE_HEIGHT - px(2.5)),
                        size(to - from, px(1.5)),
                    ),
                    theme::ui(cx, problem.role),
                ));
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

            let bookmark = self
                .bookmarks
                .iter()
                .find(|mark| mark.line == line_index.display_number());
            let is_caret_line = line == caret_point.line.get();
            let (number, color) = match bookmark {
                Some(mark) => (
                    mark.mnemonic.map_or("●".to_owned(), String::from),
                    theme::ui(cx, UiRole::Warning),
                ),
                None if is_caret_line => {
                    (line_index.display_number().to_string(), active_number_color)
                }
                None => (line_index.display_number().to_string(), number_color),
            };
            let number = shape(window, number.clone(), &[run(number.len(), color)]);
            let number_x = bounds.left() + gutter_width - GUTTER_PADDING_RIGHT - number.width;
            numbers.push((point(number_x, top), number));
            if let Some(label) = self.annotation(line) {
                let label: String = label.chars().take(annotate::ANNOTATION_CHARS).collect();
                let label = shape(window, label.clone(), &[run(label.len(), number_color)]);
                numbers.push((point(bounds.left() + GUTTER_PADDING_LEFT, top), label));
            }
            let number = line_index.display_number();
            if self
                .note_lines
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&number))
            {
                change_marks.push(fill(
                    Bounds::new(
                        point(bounds.left() + px(2.), top),
                        size(px(3.), LINE_HEIGHT),
                    ),
                    theme::ui(cx, UiRole::Warning),
                ));
            }
            if let Some(mark) = self.change_mark(line) {
                let (offset, height) = match mark {
                    changes::ChangeMark::DeletedAbove => (px(-1.5), px(3.)),
                    changes::ChangeMark::DeletedBelow => (LINE_HEIGHT - px(1.5), px(3.)),
                    _ => (px(0.), LINE_HEIGHT),
                };
                let (width, left) = if height < LINE_HEIGHT {
                    (px(7.), bounds.left() + gutter_width - px(8.))
                } else {
                    (px(3.), bounds.left() + gutter_width - px(4.))
                };
                change_marks.push(fill(
                    Bounds::new(point(left, top + offset), size(width, height)),
                    theme::ui(cx, mark.role()),
                ));
            }
            if folds.region_starting(line).is_some() {
                let collapsed = folds.is_collapsed(line);
                let marker = if collapsed { "▸" } else { "▾" };
                let marker = shape(window, marker.to_owned(), &[run(marker.len(), fold_color)]);
                let marker_x = bounds.left() + gutter_width - GUTTER_PADDING_RIGHT + px(3.);
                numbers.push((point(marker_x, top), marker));
                if collapsed {
                    let text = " … ".to_owned();
                    let placeholder = shape(window, text.clone(), &[run(text.len(), fold_color)]);
                    placeholders.push((point(x(line_end) + px(4.), top), placeholder));
                }
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
            } else if self.line_action.offered && self.line_action.hover == Some(line) {
                let glyph = shape(
                    window,
                    "+".to_owned(),
                    &[run(1, theme::ui(cx, UiRole::Accent))],
                );
                numbers.push((point(bounds.left() + px(5.), top), glyph));
            }
            lines.push((display, shaped));
        }

        let mut sticky_lines: Vec<usize> = Vec::new();
        if let Some(tree) = self.syntax.as_ref().filter(|_| self.scroll_top > 0.0) {
            for _ in 0..MAX_STICKY_LINES {
                let covered_row = first_row + sticky_lines.len();
                if covered_row >= row_count {
                    break;
                }
                let covered = self.model.folds.line_of(block_rows.text_row(covered_row));
                let scopes: Vec<usize> = tree
                    .sticky_scopes(buffer, buffer.line_start(LineIndex(covered)))
                    .into_iter()
                    .map(LineIndex::get)
                    .take(MAX_STICKY_LINES)
                    .collect();
                let settled = scopes.len() <= sticky_lines.len();
                sticky_lines = scopes;
                if settled {
                    break;
                }
            }
        }
        let mut sticky = Vec::new();
        for (row, &line) in sticky_lines.iter().enumerate() {
            let line_index = LineIndex(line);
            #[expect(clippy::cast_precision_loss, reason = "at most a few sticky lines")]
            let top = bounds.top() + LINE_HEIGHT * row as f32;
            let (line_start, line_end) =
                (buffer.line_start(line_index), buffer.line_end(line_index));
            let display = DisplayLine::new(&buffer.line(line_index), MAX_LINE_BYTES);
            let spans = self
                .syntax
                .as_ref()
                .and_then(|tree| tree.highlights(buffer, line_start..line_end).ok())
                .unwrap_or_default();
            let (runs, _) = highlight_runs(
                &spans,
                line_end,
                display.text.len(),
                |offset| {
                    display.to_display(
                        offset.get().clamp(line_start.get(), line_end.get()) - line_start.get(),
                    )
                },
                |len, kind| run(len, kind.map_or(text_color, |kind| theme::syntax(cx, kind))),
            );
            let text = shape(window, display.text.clone(), &runs);
            let number = line_index.display_number().to_string();
            let number = shape(window, number.clone(), &[run(number.len(), number_color)]);
            let number_x = bounds.left() + gutter_width - GUTTER_PADDING_RIGHT - number.width;
            sticky.push(StickyRow {
                background: fill(
                    Bounds::from_corners(
                        point(bounds.left(), top),
                        point(bounds.right() - SCROLLBAR_WIDTH, top + LINE_HEIGHT),
                    ),
                    theme::ui(cx, UiRole::PanelBackground),
                ),
                number: (point(number_x, top), number),
                text: (point(text_left - self.scroll_x, top), text),
            });
        }
        let sticky_separator = (!sticky.is_empty()).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "at most a few sticky lines")]
            let bottom = bounds.top() + LINE_HEIGHT * sticky.len() as f32;
            fill(
                Bounds::from_corners(
                    point(bounds.left(), bottom),
                    point(bounds.right() - SCROLLBAR_WIDTH, bottom + px(1.)),
                ),
                theme::ui(cx, UiRole::Border),
            )
        });

        let text_width = bounds.right() - text_left - SCROLLBAR_WIDTH;
        self.scroll_x = self.scroll_x.min((max_width - text_width / 2.).max(px(0.)));

        #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
        let fits = row_count as f64 <= visible_lines;
        let scrollbar = (!fits).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^52")]
            let total = row_count as f64 + visible_lines - 1.0;
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
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a few hundred characters at most"
        )]
        let columns =
            ((bounds.size.width - gutter_width - SCROLLBAR_WIDTH) / digit_width).max(0.0) as usize;
        self.painted = Some(PaintedLines {
            bounds,
            text_left,
            first_line_top,
            rows: painted_rows,
            blocks: painted_blocks,
            gutter_right: bounds.left() + gutter_width,
            visible_lines,
            scroll_x: self.scroll_x,
            columns,
            lines: lines.clone(),
            scrollbar,
            sticky_lines,
        });
        EditorLayout {
            text_bounds: Bounds::from_corners(
                point(bounds.left() + gutter_width, bounds.top()),
                bounds.bottom_right(),
            ),
            text_origin: point(text_left - self.scroll_x, first_line_top),
            lines,
            numbers,
            change_marks,
            backgrounds,
            caret,
            placeholders,
            sticky,
            sticky_separator,
            scrollbar: scrollbar.map(|thumb| {
                let color = theme::ui(cx, UiRole::TextFaint).opacity(0.35);
                fill(thumb, color).corner_radii(px(4.))
            }),
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_ready() {
            self.refresh_usages(cx);
        }
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

struct StickyRow {
    background: PaintQuad,
    number: (Point<Pixels>, ShapedLine),
    text: (Point<Pixels>, ShapedLine),
}

struct EditorLayout {
    text_bounds: Bounds<Pixels>,
    text_origin: Point<Pixels>,
    lines: Arc<Vec<(DisplayLine, ShapedLine)>>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    change_marks: Vec<PaintQuad>,
    backgrounds: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    placeholders: Vec<(Point<Pixels>, ShapedLine)>,
    sticky: Vec<StickyRow>,
    sticky_separator: Option<PaintQuad>,
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
            for mark in layout.change_marks.drain(..) {
                window.paint_quad(mark);
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
                    for (origin, placeholder) in &layout.placeholders {
                        paint_line(placeholder, *origin, window, cx);
                    }
                    if let Some(caret) = layout.caret.take() {
                        window.paint_quad(caret);
                    }
                },
            );
            for row in layout.sticky.drain(..) {
                window.paint_quad(row.background);
                paint_line(&row.number.1, row.number.0, window, cx);
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: layout.text_bounds,
                    }),
                    |window| paint_line(&row.text.1, row.text.0, window, cx),
                );
            }
            if let Some(separator) = layout.sticky_separator.take() {
                window.paint_quad(separator);
            }
            if let Some(scrollbar) = layout.scrollbar.take() {
                window.paint_quad(scrollbar);
            }
        });

        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let editor = self.editor.clone();
        let hovered = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hovered.is_hovered(window) {
                editor.update(cx, |editor, cx| editor.on_mouse_down(event, window, cx));
            }
        });
        let editor = self.editor.clone();
        let hovered = hitbox.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hovered.is_hovered(window);
                editor.update(cx, |editor, cx| editor.on_mouse_move(event, hovered, cx));
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

fn highlight_runs(
    spans: &[HighlightSpan],
    line_end: ByteOffset,
    text_len: usize,
    column: impl Fn(ByteOffset) -> usize,
    run: impl Fn(usize, Option<dumbj_syntax::HighlightKind>) -> TextRun,
) -> (Vec<TextRun>, usize) {
    let mut runs = Vec::new();
    let mut position = 0;
    let mut consumed = 0;
    for span in spans.iter().take_while(|span| span.range.start < line_end) {
        let (start, end) = (column(span.range.start), column(span.range.end));
        if start > position {
            runs.push(run(start - position, None));
        }
        if end > start.max(position) {
            runs.push(run(end - start.max(position), Some(span.kind)));
            position = end;
        }
        if span.range.end > line_end {
            break; // The span continues on the next line.
        }
        consumed += 1;
    }
    if text_len > position {
        runs.push(run(text_len - position, None));
    }
    (runs, consumed)
}

fn paint_line(line: &ShapedLine, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    if let Err(error) = line.paint(origin, LINE_HEIGHT, TextAlign::Left, None, window, cx) {
        tracing::warn!(%error, "failed to paint a line");
    }
}
