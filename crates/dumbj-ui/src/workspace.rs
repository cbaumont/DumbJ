//! A project window: tool-window stripe, Project tool window, editor tabs, status bar and
//! popups. It owns the key dispatcher and routes every action.

use std::path::{Path, PathBuf};
use std::time::Instant;

use dumbj_actions::{Action, Bindings, Dispatch, KeyDispatcher, Scope, ShortcutStyle};
use dumbj_settings::UiRole;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Decorations, Entity, FocusHandle, IntoElement,
    KeyDownEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, PathPromptOptions, Pixels,
    Point, Render, ResizeEdge, Size, Subscription, Window, WindowBounds, WindowDecorations,
    WindowOptions, div, prelude::*, px, size, svg,
};

use crate::editor::{EditorView, MousePressed};
use crate::editor_model::parse_line_column;
use crate::file_tree::{FileTree, OpenFile};
use crate::keys::{modifiers_from_gpui, stroke_from_gpui};
use crate::text_input::{TextChanged, TextInput};
use crate::theme;

mod navigation;
mod search_popup;

use dumbj_index::SearchScope;
use navigation::{Chooser, Indexing, NavigationHistory};
use search_popup::{SearchMode, SearchPopup};

const HEADER_HEIGHT: Pixels = px(40.);
const STRIPE_WIDTH: Pixels = px(40.);
const TOOL_WINDOW_WIDTH: Pixels = px(280.);
const TAB_BAR_HEIGHT: Pixels = px(36.);
const STATUS_BAR_HEIGHT: Pixels = px(26.);
/// The width of the invisible resize border of a client-decorated window.
const RESIZE_BORDER: Pixels = px(6.);

struct Project {
    root: PathBuf,
    name: String,
    tree: Entity<FileTree>,
}

struct Tab {
    path: PathBuf,
    title: String,
    editor: Entity<EditorView>,
}

/// Whether the Project tool window is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolWindowState {
    Visible,
    Hidden,
    /// Hidden by Hide All Tool Windows, which restores it when invoked again.
    HiddenByHideAll,
}

/// Which part of the window has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Chooser,
    Search,
    GotoLine,
    Find,
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
    project_window: ToolWindowState,
    find: Entity<TextInput>,
    find_open: bool,
    goto_line: Entity<TextInput>,
    goto_line_open: bool,
    /// The symbol index; `None` until the first indexing run has finished.
    index: Option<std::sync::Arc<dumbj_index::Index>>,
    indexing: Option<Indexing>,
    history: NavigationHistory,
    chooser: Option<Chooser>,
    search_input: Entity<TextInput>,
    search: Option<SearchPopup>,
    /// A one-off notice in the status bar, cleared by the next action.
    status_message: Option<String>,
    subscriptions: Vec<Subscription>,
}

/// What a new project window shows.
#[derive(Default)]
pub(crate) struct OpenOptions {
    pub(crate) folder: Option<PathBuf>,
    pub(crate) files: Vec<PathBuf>,
    /// Run the scroll benchmark on the first opened file.
    pub(crate) scroll_benchmark: bool,
}

/// Opens a project window, optionally on a folder and with files already open.
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
            find,
            find_open: false,
            goto_line,
            goto_line_open: false,
            index: None,
            indexing: None,
            history: NavigationHistory::default(),
            chooser: None,
            search_input,
            search: None,
            status_message: None,
            subscriptions: vec![on_find_changed, on_search_changed],
        }
    }

    fn project_window_shown(&self) -> bool {
        self.project_window == ToolWindowState::Visible
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }

    fn active_editor(&self) -> Option<Entity<EditorView>> {
        self.active_tab().map(|tab| tab.editor.clone())
    }

    fn focus_area(&self, window: &Window, cx: &App) -> FocusArea {
        let tree_focused = self
            .project
            .as_ref()
            .is_some_and(|project| project.tree.read(cx).focus_handle().is_focused(window));
        let editor_focused = self
            .active_tab()
            .is_some_and(|tab| tab.editor.read(cx).focus_handle().is_focused(window));
        if self.chooser.is_some() {
            FocusArea::Chooser
        } else if self.search.is_some() {
            FocusArea::Search
        } else if self.goto_line_open && self.goto_line.read(cx).focus_handle().is_focused(window) {
            FocusArea::GotoLine
        } else if self.find_open && self.find.read(cx).focus_handle().is_focused(window) {
            FocusArea::Find
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
        self.start_indexing(root.clone(), cx);
        self.project = Some(Project { root, name, tree });
        self.project_window = ToolWindowState::Visible;
        self.update_title(window);
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.path == path)
            .unwrap_or_else(|| {
                let title = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let editor = cx.new(|cx| EditorView::open(path.clone(), cx));
                self.subscriptions.push(cx.subscribe_in(
                    &editor,
                    window,
                    |this, _, MousePressed(stroke), window, cx| {
                        this.on_editor_mouse(*stroke, window, cx);
                    },
                ));
                // Repaint the status bar when the caret moves.
                self.subscriptions
                    .push(cx.observe(&editor, |_, _, cx| cx.notify()));
                self.tabs.push(Tab {
                    path,
                    title,
                    editor,
                });
                self.tabs.len() - 1
            });
        self.activate_tab(index, window, cx);
    }

    fn activate_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        self.active = Some(index);
        let handle = tab.editor.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        if self.find_open {
            let query = self.find.read(cx).text().to_owned();
            tab.editor
                .update(cx, |editor, cx| editor.set_search_query(&query, cx));
        }
        self.update_title(window);
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
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

    /// Focuses the editor if there is one, else the tree, else the window itself.
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
                    // One window per project: a second folder gets its own window.
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
            editor.update(cx, |editor, cx| editor.go_to(line, column, cx));
            self.goto_line_open = false;
            self.focus_fallback(window, cx);
            cx.notify();
        }
    }

    /// Whether `action` can run right now. The dispatcher skips disabled actions, so a
    /// shortcut shared by several actions reaches the one that applies, and an unusable
    /// shortcut falls through to text input.
    fn is_enabled(&self, action: Action, window: &Window, cx: &App) -> bool {
        let has_editor = self.active.is_some();
        // The search actions stay available while the search popup is open, so that
        // pressing another one switches its scope.
        let other_popup = self.goto_line_open || self.chooser.is_some();
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
            Action::GotoDeclaration => has_editor && self.index.is_some() && !popup_open,
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
            Action::GotoDeclaration => self.goto_declaration(window, cx),
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
            Action::ActivateProjectToolWindow => {
                let Some(project) = &self.project else { return };
                let tree_focus = project.tree.read(cx).focus_handle().clone();
                // IDEA: the shortcut shows and focuses the tool window, or hides it when
                // it already has focus.
                if self.project_window_shown() && tree_focus.is_focused(window) {
                    self.project_window = ToolWindowState::Hidden;
                    self.focus_fallback(window, cx);
                } else {
                    self.project_window = ToolWindowState::Visible;
                    window.focus(&tree_focus, cx);
                }
                cx.notify();
            }
            Action::HideAllToolWindows => {
                match self.project_window {
                    ToolWindowState::Visible => {
                        self.project_window = ToolWindowState::HiddenByHideAll;
                        self.focus_fallback(window, cx);
                    }
                    ToolWindowState::HiddenByHideAll => {
                        self.project_window = ToolWindowState::Visible;
                    }
                    ToolWindowState::Hidden => {}
                }
                cx.notify();
            }
            Action::GotoLine => {
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
            Action::Find => {
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

    /// Routes editor and text actions to whatever has focus.
    fn perform_in_focus(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match self.focus_area(window, cx) {
            FocusArea::Chooser => self.chooser_action(action, window, cx),
            FocusArea::Search => self.search_action(action, window, cx),
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

    /// The first displayable shortcut of `action` in the active keymap.
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
                // Without server decorations the header is the title bar.
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
        let active = self.project_window_shown();
        let icon_role = if active {
            UiRole::Text
        } else {
            UiRole::TextMuted
        };
        let button = div()
            .id("stripe-project")
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .when(active, |button| button.bg(theme::ui(cx, UiRole::Selected)))
            .hover(|style| style.bg(theme::ui(cx, UiRole::Hover)))
            .on_click(cx.listener(|this, _, window, cx| {
                // A click toggles visibility regardless of focus.
                if this.project_window_shown() {
                    this.project_window = ToolWindowState::Hidden;
                    this.focus_fallback(window, cx);
                    cx.notify();
                } else {
                    this.perform(Action::ActivateProjectToolWindow, window, cx);
                }
            }))
            .child(
                svg()
                    .path("icons/folder-tree.svg")
                    .size(px(16.))
                    .text_color(theme::ui(cx, icon_role)),
            );
        div()
            .w(STRIPE_WIDTH)
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(6.))
            .bg(theme::ui(cx, UiRole::ChromeBackground))
            .border_r_1()
            .border_color(theme::ui(cx, UiRole::Border))
            .child(button)
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

    /// What the editor area shows when no file is open: the shortcuts worth knowing.
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
            .child(div().flex_1().min_h_0().child(tab.editor.clone()))
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.active_tab();
        // The breadcrumb: project name, then the file's path inside the project.
        let breadcrumb = match (&self.project, tab) {
            (Some(project), Some(tab)) => {
                let relative = tab.path.strip_prefix(&project.root).unwrap_or(&tab.path);
                let parts = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy());
                std::iter::once(project.name.clone().into())
                    .chain(parts)
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
        right.extend(self.index_status());
        if let Some(tab) = tab {
            let editor = tab.editor.read(cx);
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

/// The window edge or corner under `position`, for resizing a client-decorated window.
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
            .map(|project| Self::render_project_window(project, cx).into_any_element());
        let stripe = self
            .project
            .is_some()
            .then(|| self.render_stripe(cx).into_any_element());
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(stripe)
            .children(project_window)
            .child(self.render_editor_area(cx));
        let popup = match (&self.chooser, &self.search) {
            (Some(chooser), _) => Some(Self::render_chooser(chooser, cx).into_any_element()),
            (None, Some(search)) => Some(self.render_search(search, cx).into_any_element()),
            (None, None) => self
                .goto_line_open
                .then(|| self.render_goto_line_popup(cx).into_any_element()),
        };
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
