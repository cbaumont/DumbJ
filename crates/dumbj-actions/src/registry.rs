use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Window,
    Editor,
    /// Caret movement and editing in whatever has focus: an editor, a text field or a list.
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    OpenFolder,
    CloseTab,
    NextTab,
    PreviousTab,
    ActivateProjectToolWindow,
    HideAllToolWindows,
    GotoLine,
    Find,
    FindNext,
    FindPrevious,
    ExtendSelection,
    GotoDeclaration,
    Back,
    Forward,
    SearchEverywhere,
    GotoClass,
    GotoFile,
    GotoSymbol,
    FileStructure,
    FindUsages,
    ShowUsages,
    ActivateFindToolWindow,
    GotoImplementation,
    GotoSuperMethod,
    TypeHierarchy,
    CallHierarchy,
    ActivateHierarchyToolWindow,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    MoveTextStart,
    MoveTextEnd,
    MovePageUp,
    MovePageDown,
    MoveWordLeft,
    MoveWordRight,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectLineStart,
    SelectLineEnd,
    SelectTextStart,
    SelectTextEnd,
    SelectPageUp,
    SelectPageDown,
    SelectWordLeft,
    SelectWordRight,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Backspace,
    Delete,
    Tab,
    Enter,
    Escape,
}

struct ActionInfo {
    action: Action,
    id: &'static str,
    title: &'static str,
    scope: Scope,
}

const fn info(action: Action, id: &'static str, title: &'static str, scope: Scope) -> ActionInfo {
    ActionInfo {
        action,
        id,
        title,
        scope,
    }
}

/// One row per action, in declaration order. A test checks the order.
const REGISTRY: &[ActionInfo] = &[
    info(
        Action::OpenFolder,
        "project.open-folder",
        "Open Folder…",
        Scope::Window,
    ),
    info(Action::CloseTab, "tabs.close", "Close Tab", Scope::Window),
    info(
        Action::NextTab,
        "tabs.next",
        "Select Next Tab",
        Scope::Window,
    ),
    info(
        Action::PreviousTab,
        "tabs.previous",
        "Select Previous Tab",
        Scope::Window,
    ),
    info(
        Action::ActivateProjectToolWindow,
        "tool-window.project",
        "Project Tool Window",
        Scope::Window,
    ),
    info(
        Action::HideAllToolWindows,
        "tool-window.hide-all",
        "Hide All Tool Windows",
        Scope::Window,
    ),
    info(
        Action::GotoLine,
        "editor.goto-line",
        "Go to Line:Column…",
        Scope::Editor,
    ),
    info(Action::Find, "editor.find", "Find…", Scope::Editor),
    info(
        Action::FindNext,
        "editor.find-next",
        "Find Next",
        Scope::Editor,
    ),
    info(
        Action::FindPrevious,
        "editor.find-previous",
        "Find Previous",
        Scope::Editor,
    ),
    info(
        Action::ExtendSelection,
        "editor.extend-selection",
        "Extend Selection",
        Scope::Editor,
    ),
    info(
        Action::GotoDeclaration,
        "navigate.goto-declaration",
        "Go to Declaration",
        Scope::Editor,
    ),
    info(Action::Back, "navigate.back", "Back", Scope::Window),
    info(
        Action::Forward,
        "navigate.forward",
        "Forward",
        Scope::Window,
    ),
    info(
        Action::SearchEverywhere,
        "navigate.search-everywhere",
        "Search Everywhere",
        Scope::Window,
    ),
    info(
        Action::GotoClass,
        "navigate.goto-class",
        "Go to Class…",
        Scope::Window,
    ),
    info(
        Action::GotoFile,
        "navigate.goto-file",
        "Go to File…",
        Scope::Window,
    ),
    info(
        Action::GotoSymbol,
        "navigate.goto-symbol",
        "Go to Symbol…",
        Scope::Window,
    ),
    info(
        Action::FileStructure,
        "navigate.file-structure",
        "File Structure",
        Scope::Editor,
    ),
    info(
        Action::FindUsages,
        "navigate.find-usages",
        "Find Usages",
        Scope::Editor,
    ),
    info(
        Action::ShowUsages,
        "navigate.show-usages",
        "Show Usages",
        Scope::Editor,
    ),
    info(
        Action::ActivateFindToolWindow,
        "tool-window.find",
        "Find Tool Window",
        Scope::Window,
    ),
    info(
        Action::GotoImplementation,
        "navigate.goto-implementation",
        "Go to Implementation(s)",
        Scope::Editor,
    ),
    info(
        Action::GotoSuperMethod,
        "navigate.goto-super-method",
        "Go to Super Method",
        Scope::Editor,
    ),
    info(
        Action::TypeHierarchy,
        "navigate.type-hierarchy",
        "Type Hierarchy",
        Scope::Editor,
    ),
    info(
        Action::CallHierarchy,
        "navigate.call-hierarchy",
        "Call Hierarchy",
        Scope::Editor,
    ),
    info(
        Action::ActivateHierarchyToolWindow,
        "tool-window.hierarchy",
        "Hierarchy",
        Scope::Window,
    ),
    info(
        Action::MoveLeft,
        "text.move-left",
        "Move Caret Left",
        Scope::Text,
    ),
    info(
        Action::MoveRight,
        "text.move-right",
        "Move Caret Right",
        Scope::Text,
    ),
    info(Action::MoveUp, "text.move-up", "Move Caret Up", Scope::Text),
    info(
        Action::MoveDown,
        "text.move-down",
        "Move Caret Down",
        Scope::Text,
    ),
    info(
        Action::MoveLineStart,
        "text.move-line-start",
        "Move Caret to Line Start",
        Scope::Text,
    ),
    info(
        Action::MoveLineEnd,
        "text.move-line-end",
        "Move Caret to Line End",
        Scope::Text,
    ),
    info(
        Action::MoveTextStart,
        "text.move-text-start",
        "Move Caret to Text Start",
        Scope::Text,
    ),
    info(
        Action::MoveTextEnd,
        "text.move-text-end",
        "Move Caret to Text End",
        Scope::Text,
    ),
    info(
        Action::MovePageUp,
        "text.move-page-up",
        "Move Caret Page Up",
        Scope::Text,
    ),
    info(
        Action::MovePageDown,
        "text.move-page-down",
        "Move Caret Page Down",
        Scope::Text,
    ),
    info(
        Action::MoveWordLeft,
        "text.move-word-left",
        "Move Caret to Previous Word",
        Scope::Text,
    ),
    info(
        Action::MoveWordRight,
        "text.move-word-right",
        "Move Caret to Next Word",
        Scope::Text,
    ),
    info(
        Action::SelectLeft,
        "text.select-left",
        "Move Caret Left with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectRight,
        "text.select-right",
        "Move Caret Right with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectUp,
        "text.select-up",
        "Move Caret Up with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectDown,
        "text.select-down",
        "Move Caret Down with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectLineStart,
        "text.select-line-start",
        "Move Caret to Line Start with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectLineEnd,
        "text.select-line-end",
        "Move Caret to Line End with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectTextStart,
        "text.select-text-start",
        "Move Caret to Text Start with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectTextEnd,
        "text.select-text-end",
        "Move Caret to Text End with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectPageUp,
        "text.select-page-up",
        "Move Caret Page Up with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectPageDown,
        "text.select-page-down",
        "Move Caret Page Down with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectWordLeft,
        "text.select-word-left",
        "Move Caret to Previous Word with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectWordRight,
        "text.select-word-right",
        "Move Caret to Next Word with Selection",
        Scope::Text,
    ),
    info(
        Action::SelectAll,
        "text.select-all",
        "Select All",
        Scope::Text,
    ),
    info(Action::Copy, "text.copy", "Copy", Scope::Text),
    info(Action::Cut, "text.cut", "Cut", Scope::Text),
    info(Action::Paste, "text.paste", "Paste", Scope::Text),
    info(
        Action::Backspace,
        "text.backspace",
        "Backspace",
        Scope::Text,
    ),
    info(Action::Delete, "text.delete", "Delete", Scope::Text),
    info(Action::Tab, "text.tab", "Tab", Scope::Text),
    info(Action::Enter, "text.enter", "Enter", Scope::Text),
    info(Action::Escape, "text.escape", "Escape", Scope::Text),
];

impl Action {
    /// Every action, in the order menus and documentation list them.
    pub fn all() -> impl Iterator<Item = Self> {
        REGISTRY.iter().map(|info| info.action)
    }

    fn info(self) -> &'static ActionInfo {
        // The registry has one row per variant in declaration order, so the discriminant
        // is the row index. `registry_matches_the_enum` checks this.
        &REGISTRY[self as usize]
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        self.info().id
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        self.info().title
    }

    #[must_use]
    pub fn scope(self) -> Scope {
        self.info().scope
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        REGISTRY
            .iter()
            .find(|info| info.id == id)
            .map(|info| info.action)
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn registry_matches_the_enum() {
        for (index, info) in REGISTRY.iter().enumerate() {
            assert_eq!(info.action as usize, index, "{} is out of order", info.id);
        }
        // The last variant must be the last row, so no variant is missing a row.
        assert_eq!(Action::Escape as usize, REGISTRY.len() - 1);
    }

    #[test]
    fn ids_are_unique_and_round_trip() {
        let ids: BTreeSet<_> = Action::all().map(Action::id).collect();
        assert_eq!(ids.len(), REGISTRY.len());
        for action in Action::all() {
            assert_eq!(Action::from_id(action.id()), Some(action));
            assert_ne!(action.title(), "");
        }
        assert_eq!(Action::from_id("nope"), None);
    }
}
