use std::path::{Path, PathBuf};
use std::time::Instant;

use dumbj_actions::{Action, Bindings, Dispatch, KeyDispatcher, Scope, ShortcutStyle};
use dumbj_settings::UiRole;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Decorations, Entity, FocusHandle, IntoElement,
    KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, PathPromptOptions, Pixels,
    Point, Render, ResizeEdge, Size, Subscription, Task, Window, WindowBounds, WindowDecorations,
    WindowOptions, div, prelude::*, px, size, svg,
};

use crate::editor::{EditorView, GutterIconClicked, MousePressed};
use crate::editor_model::parse_line_column;
use crate::file_tree::{FileTree, OpenFile};
use crate::keys::{modifiers_from_gpui, stroke_from_gpui};
use crate::text_input::{TextChanged, TextInput};
use crate::theme;

mod bookmarks;
mod branches;
mod diff;
mod editing;
mod find_in_files;
mod git_log;
mod head_index;
mod hierarchy;
mod library;
mod navigation;
mod review;
mod review_notes;
mod search_popup;
mod state;
mod usages;
mod vcs;

use bookmarks::MnemonicPrompt;
use branches::BranchPopup;
use diff::{DiffSides, DiffTab};
use dumbj_index::SearchScope;
use dumbj_settings::ProjectState;
use find_in_files::FindInFiles;
use git_log::GitWindow;
use hierarchy::HierarchyWindow;
use navigation::{Chooser, Indexing, NavigationHistory};
use review::{ReviewPrompt, ReviewWindow};
use review_notes::NotePrompt;
use search_popup::{RecentKind, SearchMode, SearchPopup};
use state::StateSave;
use usages::FindWindow;

const HEADER_HEIGHT: Pixels = px(40.);
const STRIPE_WIDTH: Pixels = px(40.);
const TOOL_WINDOW_WIDTH: Pixels = px(280.);
const TAB_BAR_HEIGHT: Pixels = px(36.);
const STATUS_BAR_HEIGHT: Pixels = px(26.);
const RESIZE_BORDER: Pixels = px(6.);

struct Project {
    root: PathBuf,
    name: String,
    tree: Entity<FileTree>,
}

struct Tab {
    path: PathBuf,
    title: String,
    /// The file's editor; a diff tab shows it on the right and shares it with the file's
    /// own tab, so both show the same unsaved text.
    editor: Entity<EditorView>,
    diff: Option<DiffTab>,
}

impl Tab {
    /// Whether `editor` holds the working file, not a revision of it.
    fn shows_working_file(&self) -> bool {
        self.diff.as_ref().is_none_or(DiffTab::shows_working_file)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolWindowState {
    Visible,
    Hidden,
    /// Hidden by Hide All Tool Windows, which restores it when invoked again.
    HiddenByHideAll,
}

/// Which tool window the left side shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftTool {
    Project,
    Review,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingFocus {
    Results,
    Hierarchy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    FindWindow,
    HierarchyWindow,
    Git,
    GitFilter,
    Chooser,
    Search,
    FindInFiles,
    ReviewPrompt,
    NotePrompt,
    BranchPopup,
    GotoLine,
    Find,
    Review,
    Tree,
    Editor,
    None,
}

pub(crate) struct Workspace {
    focus: FocusHandle,
    dispatcher: KeyDispatcher,
    bindings: Bindings,
    shortcut_style: ShortcutStyle,
    project: Option<Project>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// Whether the left tool window is shown; `left_tool` says which one it is.
    project_window: ToolWindowState,
    left_tool: LeftTool,
    review: Option<ReviewWindow>,
    review_focus: FocusHandle,
    review_prompt: Option<ReviewPrompt>,
    review_input: Entity<TextInput>,
    note_prompt: Option<NotePrompt>,
    branch_popup: Option<BranchPopup>,
    branch_input: Entity<TextInput>,
    note_input: Entity<TextInput>,
    find: Entity<TextInput>,
    find_open: bool,
    goto_line: Entity<TextInput>,
    goto_line_open: bool,
    /// The symbol index; `None` until the first indexing run has finished.
    index: Option<std::sync::Arc<dumbj_index::Index>>,
    head_index: Option<head_index::HeadIndex>,
    jars: Option<std::sync::Arc<dumbj_jars::JarIndex>>,
    indexing: Option<Indexing>,
    index_refresh: Option<Task<()>>,
    repository: Option<dumbj_vcs::Repository>,
    branch: Option<String>,
    history: NavigationHistory,
    chooser: Option<Chooser>,
    search_input: Entity<TextInput>,
    search: Option<SearchPopup>,
    find_in_files: Option<FindInFiles>,
    mnemonic_prompt: Option<MnemonicPrompt>,
    find_query: Entity<TextInput>,
    find_mask: Entity<TextInput>,
    state: ProjectState,
    /// Where `state` is saved; known once the background lookup has finished.
    state_dir: Option<PathBuf>,
    state_save: StateSave,
    find_window: Option<FindWindow>,
    find_window_focus: FocusHandle,
    hierarchy_window: Option<HierarchyWindow>,
    git_window: Option<GitWindow>,
    git_focus: FocusHandle,
    git_filter: Entity<TextInput>,
    hierarchy_window_focus: FocusHandle,
    usages_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Results that arrived since the last render; it moves focus to them.
    pending_focus: Option<PendingFocus>,
    /// A one-off notice in the status bar, cleared by the next action.
    status_message: Option<String>,
    subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub(crate) struct OpenOptions {
    pub(crate) folder: Option<PathBuf>,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) scroll_benchmark: bool,
}

pub(crate) fn open_window(
    cx: &mut App,
    bindings: Bindings,
    shortcut_style: ShortcutStyle,
    open: OpenOptions,
) {
    let OpenOptions {
        folder,
        files,
        scroll_benchmark,
    } = open;
    let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(480.), px(320.))),
        // Where the compositor cannot draw decorations (GNOME), the header acts as title bar.
        window_decorations: Some(WindowDecorations::Server),
        app_id: Some("dumbj".to_owned()),
        ..WindowOptions::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut workspace = Workspace::new(bindings, shortcut_style, cx);
            workspace.subscriptions.push(cx.observe_window_activation(
                window,
                |this: &mut Workspace, window, cx| {
                    this.on_window_activation(window, cx);
                },
            ));
            workspace
                .subscriptions
                .push(cx.on_app_quit(|this: &mut Workspace, cx| {
                    this.save_all_blocking(cx);
                    async {}
                }));
            if let Some(folder) = folder {
                workspace.set_project(folder, window, cx);
            }
            for file in files {
                workspace.open_file(file, window, cx);
            }
            if let Some(editor) = workspace.active_editor().filter(|_| scroll_benchmark) {
                editor.update(cx, |editor, _| editor.start_scroll_benchmark());
            }
            workspace.update_title(window);
            workspace
        })
    });
    if let Err(error) = opened {
        tracing::error!(%error, "cannot open a window");
    }
}

impl Workspace {
    fn new(bindings: Bindings, shortcut_style: ShortcutStyle, cx: &mut Context<Self>) -> Self {
        let find = cx.new(|cx| TextInput::new("Search", cx));
        let goto_line = cx.new(|cx| TextInput::new("Line:Column", cx));
        let search_input = cx.new(|cx| TextInput::new("Search", cx));
        let on_search_changed = cx.subscribe(&search_input, |this, _, _: &TextChanged, cx| {
            this.run_search(cx);
        });
        let review_input = cx.new(|cx| TextInput::new("Base revision or range", cx));
        let on_review_changed = cx.subscribe(&review_input, |this, _, _: &TextChanged, cx| {
            this.review_query_changed(cx);
        });
        let branch_input = cx.new(|cx| TextInput::new("Branch", cx));
        let on_branch_changed = cx.subscribe(&branch_input, |this, _, _: &TextChanged, cx| {
            this.branch_query_changed(cx);
        });
        let find_query = cx.new(|cx| TextInput::new("Text to find", cx));
        let find_mask = cx.new(|cx| TextInput::new("*.java, *.kt, !*.test.kt", cx));
        let on_find_in_files_changed = [&find_query, &find_mask].map(|input| {
            cx.subscribe(input, |this, _, _: &TextChanged, cx| {
                this.schedule_find_in_files(cx);
            })
        });
        let on_find_changed = cx.subscribe(&find, |this, find, _: &TextChanged, cx| {
            let query = find.read(cx).text().to_owned();
            if let Some(editor) = this.active_editor() {
                editor.update(cx, |editor, cx| editor.set_search_query(&query, cx));
            }
        });
        Self {
            focus: cx.focus_handle(),
            dispatcher: KeyDispatcher::new(&bindings),
            bindings,
            shortcut_style,
            project: None,
            tabs: Vec::new(),
            active: None,
            project_window: ToolWindowState::Visible,
            left_tool: LeftTool::Project,
            review: None,
            review_focus: cx.focus_handle(),
            review_prompt: None,
            review_input,
            note_prompt: None,
            branch_popup: None,
            branch_input,
            note_input: cx.new(|cx| TextInput::new("Note", cx)),
            find,
            find_open: false,
            goto_line,
            goto_line_open: false,
            index: None,
            head_index: None,
            jars: None,
            indexing: None,
            index_refresh: None,
            repository: None,
            branch: None,
            history: NavigationHistory::default(),
            chooser: None,
            search_input,
            search: None,
            find_in_files: None,
            mnemonic_prompt: None,
            find_query,
            find_mask,
            state: ProjectState::default(),
            state_dir: None,
            state_save: StateSave::Idle,
            find_window: None,
            find_window_focus: cx.focus_handle(),
            hierarchy_window: None,
            git_window: None,
            git_focus: cx.focus_handle(),
            git_filter: cx.new(|cx| {
                TextInput::new("Filter: text  author:  path:  branch:  since:  until:", cx)
            }),
            hierarchy_window_focus: cx.focus_handle(),

            usages_cancel: std::sync::Arc::default(),
            pending_focus: None,
            status_message: None,
            subscriptions: [
                on_find_changed,
                on_search_changed,
                on_review_changed,
                on_branch_changed,
            ]
            .into_iter()
            .chain(on_find_in_files_changed)
            .collect(),
        }
    }

    fn project_window_shown(&self) -> bool {
        self.project_window == ToolWindowState::Visible && !self.review_shown()
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }

    fn active_editor(&self) -> Option<Entity<EditorView>> {
        let tab = self.active_tab()?;
        let left = tab.diff.as_ref().filter(|diff| diff.left_active());
        Some(left.map_or_else(|| tab.editor.clone(), |diff| diff.left().clone()))
    }

    fn focus_area(&self, window: &Window, cx: &App) -> FocusArea {
        let tree_focused = self
            .project
            .as_ref()
            .is_some_and(|project| project.tree.read(cx).focus_handle().is_focused(window));
        let editor_focused = self.active_tab().is_some_and(|tab| {
            let focused =
                |editor: &Entity<EditorView>| editor.read(cx).focus_handle().is_focused(window);
            focused(&tab.editor) || tab.diff.as_ref().is_some_and(|diff| focused(diff.left()))
        });
        let find_window_shown = self.find_window.as_ref().is_some_and(|find| find.visible);
        if self.chooser.is_some() {
            FocusArea::Chooser
        } else if self.search.is_some() {
            FocusArea::Search
        } else if self.find_in_files.is_some() {
            FocusArea::FindInFiles
        } else if self.review_prompt.is_some() {
            FocusArea::ReviewPrompt
        } else if self.note_prompt.is_some() {
            FocusArea::NotePrompt
        } else if self.branch_popup.is_some() {
            FocusArea::BranchPopup
        } else if self.goto_line_open && self.goto_line.read(cx).focus_handle().is_focused(window) {
            FocusArea::GotoLine
        } else if self.find_open && self.find.read(cx).focus_handle().is_focused(window) {
            FocusArea::Find
        } else if find_window_shown && self.find_window_focus.is_focused(window) {
            FocusArea::FindWindow
        } else if self.git_filter_focused(window, cx) {
            FocusArea::GitFilter
        } else if self.git_window_focused(window) {
            FocusArea::Git
        } else if self.hierarchy_window.as_ref().is_some_and(|h| h.visible)
            && self.hierarchy_window_focus.is_focused(window)
        {
            FocusArea::HierarchyWindow
        } else if self.review_shown() && self.review_focus.is_focused(window) {
            FocusArea::Review
        } else if tree_focused {
            FocusArea::Tree
        } else if editor_focused {
            FocusArea::Editor
        } else {
            FocusArea::None
        }
    }

    fn update_title(&self, window: &mut Window) {
        let project = self.project.as_ref().map(|project| project.name.as_str());
        let file = self.active_tab().map(|tab| tab.title.as_str());
        let title = match (project, file) {
            (Some(project), Some(file)) => format!("{project} – {file}"),
            (Some(name), None) | (None, Some(name)) => name.to_owned(),
            (None, None) => "DumbJ".to_owned(),
        };
        window.set_window_title(&title);
    }

    fn set_project(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let tree = cx.new(|cx| FileTree::new(root.clone(), cx));
        self.subscriptions.push(cx.subscribe_in(
            &tree,
            window,
            |this, _, OpenFile(path), window, cx| this.open_file(path.clone(), window, cx),
        ));
        let handle = tree.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        self.index = None;
        self.head_index = None;
        self.start_indexing(root.clone(), cx);
        self.load_project_state(root.clone(), cx);
        self.start_jar_indexing(cx);
        self.project = Some(Project { root, name, tree });
        self.refresh_repository(cx);
        self.project_window = ToolWindowState::Visible;
        self.update_title(window);
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let opened = path.clone();
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.path == path && tab.diff.is_none())
            .unwrap_or_else(|| {
                let mut title = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                if let Some((_, commit, _)) = dumbj_vcs::split_revision_path(&path) {
                    let short: String = commit.chars().take(7).collect();
                    title = format!("{title} @{short}");
                }
                let editor = self.editor_for(&path, window, cx);
                self.tabs.push(Tab {
                    path,
                    title,
                    editor,
                    diff: None,
                });
                self.tabs.len() - 1
            });
        self.activate_tab(index, window, cx);
        self.record_recent_file(&opened, cx);
        self.refresh_bookmark_marks(cx);
    }

    /// The editor of `path`: the one a tab already shows, or a new one.
    fn editor_for(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorView> {
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.path == path && tab.shows_working_file())
        {
            return tab.editor.clone();
        }
        let editor = cx.new(|cx| EditorView::open(path.to_path_buf(), cx));
        self.subscriptions.push(cx.subscribe_in(
            &editor,
            window,
            |this, _, MousePressed(stroke), window, cx| {
                this.on_editor_mouse(*stroke, window, cx);
            },
        ));
        self.subscriptions.push(cx.subscribe_in(
            &editor,
            window,
            |this, _, GutterIconClicked(icon), window, cx| {
                this.gutter_icon_clicked(*icon, window, cx);
            },
        ));
        self.subscriptions
            .push(cx.subscribe(&editor, |this, editor, event, cx| {
                this.on_editor_event(&editor, event, cx);
            }));
        self.subscriptions
            .push(cx.observe(&editor, |_, _, cx| cx.notify()));
        self.refresh_gutter_icons(&editor, path, cx);
        self.refresh_base_text(&editor, path, cx);
        editor
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.active != Some(index) {
            self.record_current_location(cx);
            if let Some(editor) = self.active_editor() {
                editor.update(cx, EditorView::save);
            }
        }
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        self.active = Some(index);
        let marks = tab.diff.as_ref().map(DiffTab::right_marks);
        tab.editor.update(cx, |editor, cx| {
            editor.set_diff_marks(marks.unwrap_or_default(), cx);
            editor.check_disk(cx);
        });
        let handle = tab.editor.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        if self.find_open {
            let query = self.find.read(cx).text().to_owned();
            tab.editor
                .update(cx, |editor, cx| editor.set_search_query(&query, cx));
        }
        if let Some(left) = tab.diff.as_ref().map(|diff| diff.left().entity_id()) {
            self.diff_changed(left, cx);
        }
        self.update_title(window);
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        if self.active == Some(index) {
            self.record_current_location(cx);
        }
        let closed = self.tabs.remove(index);
        closed.editor.update(cx, EditorView::save);
        let active = self.active.unwrap_or(0);
        let next = if index < active { active - 1 } else { active };
        if self.tabs.is_empty() {
            self.active = None;
            self.find_open = false;
            self.focus_fallback(window, cx);
            self.update_title(window);
            cx.notify();
        } else {
            self.activate_tab(next.min(self.tabs.len() - 1), window, cx);
        }
    }

    fn focus_fallback(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            let handle = tab.editor.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        } else if let Some(project) = self
            .project
            .as_ref()
            .filter(|_| self.project_window_shown())
        {
            let handle = project.tree.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }

    fn prompt_for_folder(window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let folder = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => {
                    tracing::error!(%error, "the folder chooser failed");
                    None
                }
            };
            let Some(folder) = folder else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.project.is_none() {
                    this.set_project(folder, window, cx);
                } else {
                    let (bindings, style) = (this.bindings.clone(), this.shortcut_style);
                    cx.defer(move |cx| {
                        open_window(
                            cx,
                            bindings,
                            style,
                            OpenOptions {
                                folder: Some(folder),
                                ..OpenOptions::default()
                            },
                        );
                    });
                }
            });
        })
        .detach();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.set_search_query("", cx));
        }
        self.focus_fallback(window, cx);
        cx.notify();
    }

    fn submit_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = parse_line_column(self.goto_line.read(cx).text());
        if let (Some((line, column)), Some(editor)) = (target, self.active_editor()) {
            self.record_current_location(cx);
            editor.update(cx, |editor, cx| editor.go_to(line, column, cx));
            self.goto_line_open = false;
            self.focus_fallback(window, cx);
            cx.notify();
        }
    }

    /// Whether `action` can run right now. The dispatcher skips disabled actions, so a
    /// shortcut shared by several actions reaches the one that applies, and an unusable
    /// shortcut falls through to text input.
    /// Whether one of the git popups (review range, note, branches) is open.
    fn vcs_popup_open(&self) -> bool {
        self.review_prompt.is_some() || self.note_prompt.is_some() || self.branch_popup.is_some()
    }

    fn is_enabled(&self, action: Action, window: &Window, cx: &App) -> bool {
        let has_editor = self.active.is_some();
        // The search actions stay available while the search popup is open, so that
        // pressing another one switches its scope.
        let other_popup = self.goto_line_open
            || self.chooser.is_some()
            || self.find_in_files.is_some()
            || self.mnemonic_prompt.is_some()
            || self.vcs_popup_open();
        let popup_open = other_popup || self.search.is_some();
        match action {
            Action::OpenFolder => !popup_open,
            Action::CloseTab => has_editor && !popup_open,
            Action::NextTab | Action::PreviousTab => self.tabs.len() > 1 && !popup_open,
            Action::ActivateProjectToolWindow | Action::HideAllToolWindows => {
                self.project.is_some() && !popup_open
            }
            Action::SearchEverywhere
            | Action::GotoClass
            | Action::GotoFile
            | Action::GotoSymbol => self.index.is_some() && !other_popup,
            Action::FileStructure => has_editor && self.index.is_some() && !other_popup,
            Action::RecentFiles | Action::RecentLocations => {
                self.project.is_some()
                    && !(self.goto_line_open
                        || self.chooser.is_some()
                        || self.find_in_files.is_some()
                        || self.vcs_popup_open())
            }
            Action::FindInPath => {
                self.project.is_some()
                    && !(self.goto_line_open
                        || self.chooser.is_some()
                        || self.search.is_some()
                        || self.mnemonic_prompt.is_some()
                        || self.vcs_popup_open())
            }
            Action::ToggleBookmark | Action::ToggleBookmarkWithMnemonic => {
                has_editor && self.project.is_some() && !popup_open
            }
            Action::ShowBookmarks => self.project.is_some() && !popup_open,
            _ if action.toggle_bookmark_digit().is_some() => {
                has_editor && self.project.is_some() && !popup_open
            }
            _ if action.goto_bookmark_digit().is_some() => {
                // Without such a bookmark the shortcut falls through, as IDEA's does.
                !popup_open
                    && action
                        .goto_bookmark_digit()
                        .is_some_and(|digit| self.state.bookmark_with_mnemonic(digit).is_some())
            }
            Action::FindUsages
            | Action::ShowUsages
            | Action::GotoDeclaration
            | Action::GotoImplementation
            | Action::GotoSuperMethod
            | Action::TypeHierarchy
            | Action::CallHierarchy => has_editor && self.index.is_some() && !popup_open,
            Action::ShowDiff => {
                !popup_open
                    && self.focus_area(window, cx) == FocusArea::Tree
                    && self.diff_target(cx).is_some()
            }
            // Shares its shortcut with Show Diff, which the Project tree takes.
            Action::DuplicateLine => {
                has_editor && !popup_open && self.focus_area(window, cx) != FocusArea::Tree
            }
            Action::NextDiff | Action::PreviousDiff | Action::FocusOppositeDiffPane => {
                self.active_diff().is_some() && !popup_open
            }
            Action::StartReview | Action::ActivateGitToolWindow | Action::GitBranches => {
                self.repository.is_some() && !popup_open
            }
            Action::ActivateReviewToolWindow | Action::NextUnviewedFile => {
                self.review.is_some() && !popup_open
            }
            Action::ToggleFileViewed => self.can_mark_viewed(window) && !popup_open,
            Action::AddReviewNote => self.can_add_note() && !popup_open,
            Action::ExportReviewNotes => self.has_review_notes() && !popup_open,
            Action::ActivateFindToolWindow => self.find_window.is_some() && !popup_open,
            Action::ActivateHierarchyToolWindow => self.hierarchy_window.is_some() && !popup_open,
            Action::ShowFileHistory | Action::ShowHistoryForSelection | Action::Annotate => {
                !popup_open
                    && self.repository.is_some()
                    && self.active_tab().is_some_and(Tab::shows_working_file)
            }
            Action::Find if self.focus_area(window, cx) == FocusArea::Git => !popup_open,
            Action::Back => self.history.can_go_back() && !popup_open,
            Action::Forward => self.history.can_go_forward() && !popup_open,
            Action::FindNext | Action::FindPrevious => {
                !popup_open
                    && self
                        .active_tab()
                        .is_some_and(|tab| !tab.editor.read(cx).search_query().is_empty())
            }
            _ => match action.scope() {
                Scope::Window => true,
                Scope::Editor => has_editor && !popup_open,
                Scope::Text => self.focus_area(window, cx) != FocusArea::None,
            },
        }
    }

    /// The actions that can run right now, computed before the dispatcher is borrowed.
    fn enabled_actions(&self, window: &Window, cx: &App) -> Vec<Action> {
        Action::all()
            .filter(|action| self.is_enabled(*action, window, cx))
            .collect()
    }

    fn perform(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!(%action, "perform");
        self.status_message = None;
        match action {
            Action::SearchEverywhere => {
                self.open_search(SearchMode::Project(SearchScope::All), window, cx);
            }
            Action::GotoClass => {
                self.open_search(SearchMode::Project(SearchScope::Classes), window, cx);
            }
            Action::GotoFile => {
                self.open_search(SearchMode::Project(SearchScope::Files), window, cx);
            }
            Action::GotoSymbol => {
                self.open_search(SearchMode::Project(SearchScope::Symbols), window, cx);
            }
            Action::FileStructure => self.open_file_structure(window, cx),
            Action::FindInPath => self.open_find_in_files(window, cx),
            Action::ToggleBookmark => self.toggle_bookmark(cx),
            Action::ToggleBookmarkWithMnemonic => self.open_mnemonic_prompt(window, cx),
            Action::ShowBookmarks => self.show_bookmarks(window, cx),
            _ if action.toggle_bookmark_digit().is_some() => {
                if let Some(digit) = action.toggle_bookmark_digit() {
                    self.toggle_bookmark_mnemonic(digit, cx);
                }
            }
            _ if action.goto_bookmark_digit().is_some() => {
                if let Some(digit) = action.goto_bookmark_digit() {
                    self.goto_bookmark(digit, window, cx);
                }
            }
            Action::RecentFiles => {
                self.open_search(SearchMode::Recent(RecentKind::Files), window, cx);
            }
            Action::RecentLocations => {
                self.open_search(SearchMode::Recent(RecentKind::Locations), window, cx);
            }
            Action::FindUsages => self.find_usages(false, cx),
            Action::ShowUsages => self.find_usages(true, cx),
            Action::ActivateFindToolWindow => self.toggle_find_window(window, cx),
            Action::GotoImplementation => self.goto_implementation(window, cx),
            Action::GotoSuperMethod => self.goto_super_method(window, cx),
            Action::TypeHierarchy => self.type_hierarchy(cx),
            Action::CallHierarchy => self.call_hierarchy(cx),
            Action::ActivateHierarchyToolWindow => self.toggle_hierarchy_window(window, cx),
            Action::ActivateGitToolWindow => self.toggle_git_window(window, cx),
            Action::GitBranches => self.open_branch_popup(window, cx),
            Action::Annotate => self.toggle_annotate(cx),
            Action::ShowFileHistory | Action::ShowHistoryForSelection => {
                self.show_file_history(action == Action::ShowHistoryForSelection, window, cx);
            }
            Action::Find if self.focus_area(window, cx) == FocusArea::Git => {
                self.focus_git_filter(window, cx);
            }
            Action::GotoDeclaration => self.goto_declaration(window, cx),
            Action::ShowDiff => {
                if let Some(path) = self.diff_target(cx) {
                    self.show_diff(DiffSides::with_head_commit(path), window, cx);
                }
            }
            Action::NextDiff | Action::PreviousDiff => {
                self.go_to_difference(action == Action::NextDiff, window, cx);
            }
            Action::StartReview => self.open_review_prompt(window, cx),
            Action::ActivateReviewToolWindow => self.toggle_review_window(window, cx),
            Action::ToggleFileViewed => self.toggle_file_viewed(window, cx),
            Action::AddReviewNote => self.open_note_prompt(window, cx),
            Action::ExportReviewNotes => self.export_review_notes(cx),
            Action::NextUnviewedFile => self.next_unviewed_file(window, cx),
            Action::FocusOppositeDiffPane => self.focus_opposite_diff_pane(window, cx),
            Action::Back => self.go_back(window, cx),
            Action::Forward => self.go_forward(window, cx),
            Action::OpenFolder => Self::prompt_for_folder(window, cx),
            Action::CloseTab => {
                if let Some(active) = self.active {
                    self.close_tab(active, window, cx);
                }
            }
            Action::NextTab | Action::PreviousTab => {
                let (count, active) = (self.tabs.len(), self.active.unwrap_or(0));
                let step = if action == Action::NextTab {
                    1
                } else {
                    count - 1
                };
                self.activate_tab((active + step) % count, window, cx);
            }
            Action::ActivateProjectToolWindow => self.toggle_project_window(window, cx),
            Action::HideAllToolWindows => self.hide_all_tool_windows(window, cx),
            Action::GotoLine => self.open_goto_line(window, cx),
            Action::Find => self.open_find(window, cx),
            Action::FindNext | Action::FindPrevious => {
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |editor, cx| {
                        editor.find_next(action == Action::FindNext, cx);
                    });
                }
            }
            _ => self.perform_in_focus(action, window, cx),
        }
    }

    /// Shows and focuses the Project tool window, or hides it when it already has focus
    /// (IDEA's behaviour for a tool-window shortcut).
    fn toggle_project_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = &self.project else { return };
        let tree_focus = project.tree.read(cx).focus_handle().clone();
        if self.project_window_shown() && tree_focus.is_focused(window) {
            self.project_window = ToolWindowState::Hidden;
            self.focus_fallback(window, cx);
        } else {
            self.left_tool = LeftTool::Project;
            self.project_window = ToolWindowState::Visible;
            window.focus(&tree_focus, cx);
        }
        cx.notify();
    }

    fn hide_all_tool_windows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hiding = self.project_window == ToolWindowState::Visible
            || self.find_window.as_ref().is_some_and(|find| find.visible)
            || self.hierarchy_window.as_ref().is_some_and(|h| h.visible)
            || self.git_window.as_ref().is_some_and(|git| git.visible);
        let bottom = [
            self.find_window
                .as_mut()
                .map(|w| (&mut w.visible, &mut w.hidden_by_hide_all)),
            self.hierarchy_window
                .as_mut()
                .map(|w| (&mut w.visible, &mut w.hidden_by_hide_all)),
            self.git_window
                .as_mut()
                .map(|w| (&mut w.visible, &mut w.hidden_by_hide_all)),
        ];
        for (visible, hidden_by_hide_all) in bottom.into_iter().flatten() {
            if hiding && *visible {
                (*visible, *hidden_by_hide_all) = (false, true);
            } else if !hiding && *hidden_by_hide_all {
                (*visible, *hidden_by_hide_all) = (true, false);
            }
        }
        match (self.project_window, hiding) {
            (ToolWindowState::Visible, _) => {
                self.project_window = ToolWindowState::HiddenByHideAll;
            }
            (ToolWindowState::HiddenByHideAll, false) => {
                self.project_window = ToolWindowState::Visible;
            }
            (ToolWindowState::HiddenByHideAll, true) | (ToolWindowState::Hidden, _) => {}
        }
        if hiding {
            self.focus_fallback(window, cx);
        }
        cx.notify();
    }

    fn open_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let (line, column) = editor.read(cx).caret_line_column();
        self.goto_line.update(cx, |input, cx| {
            input.set_text(&format!("{line}:{column}"), true, cx);
        });
        self.goto_line_open = true;
        let handle = self.goto_line.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else {
            return;
        };
        let seed = editor.read(cx).selection_for_search();
        self.find.update(cx, |input, cx| match seed {
            Some(seed) => input.set_text(&seed, true, cx),
            None => input.select_all(cx),
        });
        let query = self.find.read(cx).text().to_owned();
        editor.update(cx, |editor, cx| editor.set_search_query(&query, cx));
        self.find_open = true;
        let handle = self.find.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    fn perform_in_focus(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match self.focus_area(window, cx) {
            FocusArea::FindWindow => self.find_window_action(action, window, cx),
            FocusArea::HierarchyWindow => self.hierarchy_window_action(action, window, cx),
            FocusArea::Git => self.git_window_action(action, window, cx),
            FocusArea::GitFilter => self.git_filter_action(action, window, cx),
            FocusArea::Chooser => self.chooser_action(action, window, cx),
            FocusArea::Search => self.search_action(action, window, cx),
            FocusArea::FindInFiles => self.find_in_files_action(action, window, cx),
            FocusArea::ReviewPrompt => self.review_prompt_action(action, window, cx),
            FocusArea::NotePrompt => self.note_prompt_action(action, window, cx),
            FocusArea::BranchPopup => self.branch_popup_action(action, window, cx),
            FocusArea::Review => self.review_window_action(action, window, cx),
            FocusArea::GotoLine => match action {
                Action::Enter => self.submit_goto_line(window, cx),
                Action::Escape => {
                    self.goto_line_open = false;
                    self.focus_fallback(window, cx);
                    cx.notify();
                }
                _ => {
                    self.goto_line
                        .update(cx, |input, cx| input.handle_action(action, window, cx));
                }
            },
            FocusArea::Find => match action {
                Action::Enter => self.perform(Action::FindNext, window, cx),
                Action::Escape => self.close_find(window, cx),
                _ => {
                    self.find
                        .update(cx, |input, cx| input.handle_action(action, window, cx));
                }
            },
            FocusArea::Tree => {
                let Some(project) = &self.project else { return };
                let handled = project
                    .tree
                    .update(cx, |tree, cx| tree.handle_action(action, cx));
                if !handled && action == Action::Escape {
                    self.focus_fallback(window, cx);
                }
            }
            FocusArea::Editor => {
                if action == Action::Escape && self.find_open {
                    self.close_find(window, cx);
                }
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |editor, cx| editor.handle_action(action, cx));
                }
            }
            FocusArea::None => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stroke) = stroke_from_gpui(&event.keystroke) else {
            return;
        };
        if self.handle_bookmark_key(&event.keystroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        let enabled = self.enabled_actions(window, cx);
        let dispatch = self
            .dispatcher
            .key_down(stroke, |action| enabled.contains(&action));
        tracing::trace!(key = ?event.keystroke, ?dispatch, "key down");
        match dispatch {
            Dispatch::Action(action) => {
                cx.stop_propagation();
                self.perform(action, window, cx);
            }
            Dispatch::Pending | Dispatch::ChordCancelled => {
                cx.stop_propagation();
                cx.notify();
            }
            Dispatch::Unhandled => {}
        }
    }

    fn on_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = modifiers_from_gpui(event.modifiers);
        tracing::trace!(?modifiers, "modifiers changed");
        let enabled = self.enabled_actions(window, cx);
        let action = self
            .dispatcher
            .modifiers_changed(modifiers, Instant::now(), |action| {
                enabled.contains(&action)
            });
        if let Some(action) = action {
            self.perform(action, window, cx);
        }
    }

    fn shortcut(&self, action: Action) -> Option<String> {
        self.bindings
            .shortcuts(action)
            .iter()
            .find_map(|s| s.display(self.shortcut_style))
    }

    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let title = self
            .project
            .as_ref()
            .map_or("Open Folder…", |project| project.name.as_str());
        let project_button = div()
            .id("project-name")
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(
                cx.listener(|this, _, window, cx| this.perform(Action::OpenFolder, window, cx)),
            )
            .child(title.to_owned());
        let control = |id: &'static str, icon: &'static str| {
            div()
                .id(id)
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.))
                        .text_color(theme::ui(cx, UiRole::TextMuted)),
                )
        };
        let controls = client_decorated.then(|| {
            div()
                .flex()
                .gap(px(4.))
                .child(
                    control("minimize", "icons/minus.svg")
                        .on_click(|_, window, _| window.minimize_window()),
                )
                .child(
                    control("maximize", "icons/square.svg")
                        .on_click(|_, window, _| window.zoom_window()),
                )
                .child(
                    control("close", "icons/x.svg").on_click(|_, window, _| window.remove_window()),
                )
        });
        div()
            .id("header")
            .h(HEADER_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(10.))
            .bg(theme::ui(cx, UiRole::ChromeBackground))
            .border_b_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .text_color(theme::ui(cx, UiRole::Text))
            .when(client_decorated, |header| {
                header
                    .on_mouse_down(MouseButton::Left, |event: &MouseDownEvent, window, _| {
                        if event.click_count == 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .on_mouse_down(MouseButton::Right, |event: &MouseDownEvent, window, _| {
                        window.show_window_menu(event.position);
                    })
            })
            .child(project_button)
            .children(controls)
    }

    fn render_stripe(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, icon: &'static str, tool: LeftTool| {
            let active = self.project_window == ToolWindowState::Visible && self.left_tool == tool;
            let icon_role = if active {
                UiRole::Text
            } else {
                UiRole::TextMuted
            };
            div()
                .id(id)
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .when(active, |button| button.bg(theme::ui(cx, UiRole::Selected)))
                .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    // A click toggles visibility regardless of focus.
                    let shown =
                        this.project_window == ToolWindowState::Visible && this.left_tool == tool;
                    if shown {
                        this.project_window = ToolWindowState::Hidden;
                        this.focus_fallback(window, cx);
                        cx.notify();
                    } else {
                        let action = match tool {
                            LeftTool::Project => Action::ActivateProjectToolWindow,
                            LeftTool::Review => Action::ActivateReviewToolWindow,
                        };
                        this.perform(action, window, cx);
                    }
                }))
                .child(
                    svg()
                        .path(icon)
                        .size(px(16.))
                        .text_color(theme::ui(cx, icon_role)),
                )
        };
        let review = self
            .review
            .is_some()
            .then(|| button("stripe-review", "icons/git-compare.svg", LeftTool::Review));
        div()
            .w(STRIPE_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.))
            .pt(px(6.))
            .bg(theme::ui(cx, UiRole::ChromeBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(button(
                "stripe-project",
                "icons/folder-tree.svg",
                LeftTool::Project,
            ))
            .children(review)
    }

    fn render_project_window(project: &Project, cx: &mut Context<Self>) -> impl IntoElement {
        let title = div()
            .h(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .child("Project");
        div()
            .w(TOOL_WINDOW_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(title)
            .child(div().flex_1().min_h_0().child(project.tree.clone()))
    }

    fn render_tab(&self, index: usize, tab: &Tab, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active == Some(index);
        let close = div()
            .id(("tab-close", index))
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|style| style.bg(theme::ui(cx, UiRole::Selected)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)))
            .child(
                svg()
                    .path("icons/x.svg")
                    .size(px(12.))
                    .text_color(theme::ui(cx, UiRole::TextFaint)),
            );
        let text_role = if active {
            UiRole::Text
        } else {
            UiRole::TextMuted
        };
        div()
            .id(("tab", index))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(14.))
            .pr(px(8.))
            .border_b_2()
            .border_color(if active {
                theme::ui(cx, UiRole::Accent)
            } else {
                gpui::transparent_black()
            })
            .when(active, |tab| tab.bg(theme::ui(cx, UiRole::Background)))
            .when(!active, |tab| {
                tab.hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            })
            .text_color(theme::ui(cx, text_role))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| this.activate_tab(index, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| this.close_tab(index, window, cx)),
            )
            .child(tab.title.clone())
            .when(tab.editor.read(cx).is_modified(), |tab| {
                tab.child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(theme::ui(cx, UiRole::Info)),
                )
            })
            .child(close)
    }

    fn render_find_bar(
        &self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (current, total) = editor.read(cx).search_status();
        let has_query = !self.find.read(cx).text().is_empty();
        let (status, role) = match (has_query, current, total) {
            (false, _, _) => (String::new(), UiRole::TextFaint),
            (true, _, 0) => ("0 results".to_owned(), UiRole::Error),
            (true, Some(current), total) => (format!("{current}/{total}"), UiRole::TextMuted),
            (true, None, total) => (format!("{total} results"), UiRole::TextMuted),
        };
        let field = div()
            .w(px(320.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(4.))
            .bg(theme::ui(cx, UiRole::InputBackground))
            .text_color(theme::ui(cx, UiRole::Text))
            .child(
                svg()
                    .path("icons/search.svg")
                    .size(px(13.))
                    .flex_none()
                    .text_color(theme::ui(cx, UiRole::TextFaint)),
            )
            .child(self.find.clone());
        div()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(10.))
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_b_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(field)
            .child(div().text_color(theme::ui(cx, role)).child(status))
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hints = [
            Action::ActivateProjectToolWindow,
            Action::GotoLine,
            Action::Find,
            Action::CloseTab,
        ];
        let rows = hints.into_iter().filter_map(|action| {
            let shortcut = self.shortcut(action)?;
            Some(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(div().w(px(200.)).flex().justify_end().child(action.title()))
                    .child(
                        div()
                            .text_color(theme::ui(cx, UiRole::Info))
                            .child(shortcut),
                    ),
            )
        });
        let open_folder = self.project.is_none().then(|| {
            div()
                .id("open-folder")
                .mb(px(12.))
                .px(px(14.))
                .py(px(6.))
                .rounded(px(6.))
                .bg(theme::ui(cx, UiRole::Selected))
                .text_color(theme::ui(cx, UiRole::Text))
                .cursor(CursorStyle::PointingHand)
                .on_click(
                    cx.listener(|this, _, window, cx| this.perform(Action::OpenFolder, window, cx)),
                )
                .child(Action::OpenFolder.title())
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .children(open_folder)
            .children(rows)
    }

    fn render_editor_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(tab) = self.active_tab() else {
            return div()
                .flex_1()
                .min_w_0()
                .bg(theme::ui(cx, UiRole::Background))
                .child(self.render_empty_state(cx));
        };
        let tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| self.render_tab(index, tab, cx).into_any_element())
            .collect();
        let tab_bar = div()
            .id("tab-bar")
            .h(TAB_BAR_HEIGHT)
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(theme::ui(cx, UiRole::PanelBackground))
            .border_b_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .children(tabs);
        let conflict_bar = tab
            .editor
            .read(cx)
            .has_conflict()
            .then(|| Self::render_conflict_bar(&tab.editor, cx).into_any_element());
        let find_bar = self
            .find_open
            .then(|| self.render_find_bar(&tab.editor, cx));
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(tab_bar)
            .children(find_bar)
            .children(conflict_bar)
            .child(div().flex_1().min_h_0().child(match &tab.diff {
                Some(diff) => Self::render_diff(tab, diff, cx).into_any_element(),
                None => tab.editor.clone().into_any_element(),
            }))
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.active_tab();
        let library = tab.and_then(|tab| dumbj_jars::split_virtual_path(&tab.path));
        let breadcrumb = match (&self.project, tab) {
            _ if library.is_some() => library.map_or_else(String::new, |(jar, entry)| {
                let jar = jar
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                std::iter::once(jar)
                    .chain(entry.split('/').map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join(" › ")
            }),
            (Some(project), Some(tab)) => {
                let at_commit = dumbj_vcs::split_revision_path(&tab.path);
                let relative = match &at_commit {
                    Some((_, _, relative)) => relative,
                    None => tab.path.strip_prefix(&project.root).unwrap_or(&tab.path),
                };
                let parts = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy());
                let commit = at_commit
                    .iter()
                    .map(|(_, commit, _)| format!("@{}", &commit[..commit.len().min(7)]).into());
                std::iter::once(project.name.clone().into())
                    .chain(parts)
                    .chain(commit)
                    .collect::<Vec<_>>()
                    .join(" › ")
            }
            (Some(project), None) => project.name.clone(),
            (None, Some(tab)) => tab.path.display().to_string(),
            (None, None) => String::new(),
        };
        let mut right: Vec<String> = Vec::new();
        if self.dispatcher.is_chord_pending() {
            right.push("Waiting for second key…".to_owned());
        }
        right.extend(self.status_message.clone());
        right.extend(self.note_at_caret(cx));
        right.extend(self.index_status());
        right.extend(self.head_index_status());
        right.extend(self.branch.clone());
        if let Some(editor) = self.active_editor() {
            let editor = editor.read(cx);
            let (line, column) = editor.caret_line_column();
            right.push(format!("{line}:{column}"));
            right.push(
                editor
                    .language()
                    .map_or("Plain text", |language| language.name())
                    .to_owned(),
            );
        }
        div()
            .h(STATUS_BAR_HEIGHT)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .bg(theme::ui(cx, UiRole::ChromeBackground))
            .border_t_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .text_color(theme::ui(cx, UiRole::TextMuted))
            .text_size(px(12.))
            .child(div().truncate().child(breadcrumb))
            .child(div().flex().flex_none().gap(px(18.)).children(right))
    }

    fn render_goto_line_popup(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let valid = parse_line_column(self.goto_line.read(cx).text()).is_some();
        let border = if valid { UiRole::Accent } else { UiRole::Error };
        let popup = div()
            .w(px(340.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(theme::ui(cx, UiRole::PopupBackground))
            .border_1()
            .border_color(theme::ui(cx, UiRole::PopupBorder))
            .shadow_lg()
            .text_color(theme::ui(cx, UiRole::Text))
            .child(
                div()
                    .text_color(theme::ui(cx, UiRole::TextMuted))
                    .child(Action::GotoLine.title()),
            )
            .child(
                div()
                    .flex()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .bg(theme::ui(cx, UiRole::InputBackground))
                    .border_1()
                    .border_color(theme::ui(cx, border))
                    .child(self.goto_line.clone()),
            );
        // A full-window layer: it centres the popup and swallows clicks outside it.
        div()
            .id("popup-layer")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(140.))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.goto_line_open = false;
                    this.focus_fallback(window, cx);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(popup),
            )
    }
}

impl Workspace {
    fn render_bottom_windows(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let hierarchy = self
            .hierarchy_window
            .as_ref()
            .filter(|h| h.visible)
            .map(|h| {
                self.render_hierarchy_window(h, window, cx)
                    .into_any_element()
            });
        let git = self
            .git_window
            .as_ref()
            .filter(|git| git.visible)
            .map(|git| self.render_git_window(git, window, cx).into_any_element());
        let find = self
            .find_window
            .as_ref()
            .filter(|find| find.visible)
            .map(|find| self.render_find_window(find, window, cx).into_any_element());
        hierarchy.into_iter().chain(git).chain(find).collect()
    }

    /// The open popup; when several are, the one that takes the keys.
    fn render_popup(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(chooser) = &self.chooser {
            Some(Self::render_chooser(chooser, cx).into_any_element())
        } else if let Some(search) = &self.search {
            Some(self.render_search(search, cx).into_any_element())
        } else if let Some(open) = &self.find_in_files {
            Some(
                self.render_find_in_files(open, window, cx)
                    .into_any_element(),
            )
        } else if let Some(prompt) = &self.mnemonic_prompt {
            Some(self.render_mnemonic_prompt(prompt, cx).into_any_element())
        } else if let Some(prompt) = &self.review_prompt {
            Some(self.render_review_prompt(prompt, cx).into_any_element())
        } else if let Some(prompt) = &self.note_prompt {
            Some(self.render_note_prompt(prompt, cx).into_any_element())
        } else if let Some(popup) = &self.branch_popup {
            Some(self.render_branch_popup(popup, cx).into_any_element())
        } else {
            self.goto_line_open
                .then(|| self.render_goto_line_popup(cx).into_any_element())
        }
    }
}

fn resize_edge(position: Point<Pixels>, window_size: Size<Pixels>) -> Option<ResizeEdge> {
    let (left, top) = (position.x < RESIZE_BORDER, position.y < RESIZE_BORDER);
    let right = position.x > window_size.width - RESIZE_BORDER;
    let bottom = position.y > window_size.height - RESIZE_BORDER;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, ..) => Some(ResizeEdge::Left),
        (_, true, ..) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let project_window = self
            .project
            .as_ref()
            .filter(|_| self.project_window_shown())
            .map(|project| Self::render_project_window(project, cx).into_any_element())
            .or_else(|| {
                let review = self.review.as_ref().filter(|_| self.review_shown())?;
                Some(
                    self.render_review_window(review, window, cx)
                        .into_any_element(),
                )
            });
        let stripe = self
            .project
            .is_some()
            .then(|| self.render_stripe(cx).into_any_element());
        let bottom_windows = self.render_bottom_windows(window, cx);
        if let Some(pending) = self.pending_focus.take() {
            let focus = match pending {
                PendingFocus::Hierarchy => self.hierarchy_window_focus.clone(),
                PendingFocus::Results if self.chooser.is_some() => self.focus.clone(),
                PendingFocus::Results => self.find_window_focus.clone(),
            };
            window.focus(&focus, cx);
        }
        let main = div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(project_window)
            .child(self.render_editor_area(cx));
        let body = div().flex_1().min_h_0().flex().children(stripe).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(main)
                .children(bottom_windows),
        );
        let popup = self.render_popup(window, cx);
        div()
            .id("workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::ui(cx, UiRole::Background))
            .text_color(theme::ui(cx, UiRole::Text))
            .text_size(px(13.))
            .capture_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .when(client_decorated, |root| {
                root.capture_any_mouse_down(|event: &MouseDownEvent, window, cx| {
                    let edge = resize_edge(event.position, window.viewport_size());
                    if let (MouseButton::Left, Some(edge)) = (event.button, edge) {
                        window.start_window_resize(edge);
                        cx.stop_propagation();
                    }
                })
            })
            .child(self.render_header(window, cx))
            .child(body)
            .child(self.render_status_bar(cx))
            .children(popup)
    }
}

/// The project folder and the files to open for the paths given on the command line: the
/// first folder is the project; without one, the folder of the first file is.
pub(crate) fn resolve_cli_paths(paths: &[PathBuf]) -> (Option<PathBuf>, Vec<PathBuf>) {
    let absolute: Vec<PathBuf> = paths
        .iter()
        .map(|path| std::path::absolute(path).unwrap_or_else(|_| path.clone()))
        .collect();
    let files: Vec<PathBuf> = absolute
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    let folder = absolute
        .iter()
        .find(|path| path.is_dir())
        .cloned()
        .or_else(|| {
            files
                .first()
                .and_then(|file| file.parent().map(Path::to_path_buf))
        });
    (folder, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_edges_cover_the_border() {
        let window = size(px(800.), px(600.));
        let edge = |x: f32, y: f32| resize_edge(Point { x: px(x), y: px(y) }, window);
        assert_eq!(edge(2., 2.), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(798., 2.), Some(ResizeEdge::TopRight));
        assert_eq!(edge(2., 598.), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge(798., 598.), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(2., 300.), Some(ResizeEdge::Left));
        assert_eq!(edge(798., 300.), Some(ResizeEdge::Right));
        assert_eq!(edge(400., 2.), Some(ResizeEdge::Top));
        assert_eq!(edge(400., 598.), Some(ResizeEdge::Bottom));
        assert_eq!(edge(400., 300.), None);
        assert_eq!(edge(10., 10.), None);
    }

    #[test]
    fn cli_paths_resolve_to_a_project_and_files() {
        let dir = std::env::temp_dir().join(format!("dumbj-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("A.java");
        std::fs::write(&file, "").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let nested = sub.join("B.kt");
        std::fs::write(&nested, "").unwrap();
        let resolve = |paths: &[&PathBuf]| {
            resolve_cli_paths(&paths.iter().map(|path| (*path).clone()).collect::<Vec<_>>())
        };
        assert_eq!(resolve(&[&dir]), (Some(dir.clone()), Vec::new()));
        assert_eq!(resolve(&[&file]), (Some(dir.clone()), vec![file.clone()]));
        assert_eq!(
            resolve(&[&nested, &dir, &file]),
            (Some(dir.clone()), vec![nested.clone(), file.clone()]),
            "an explicit folder wins over the first file's folder"
        );
        assert_eq!(
            resolve(&[&nested]),
            (Some(sub.clone()), vec![nested.clone()])
        );
        assert_eq!(resolve(&[&dir.join("missing")]), (None, Vec::new()));
        assert_eq!(resolve(&[]), (None, Vec::new()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
