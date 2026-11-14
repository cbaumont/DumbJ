//! The registry of DumbJ actions. Menus, Find Action and the keymap all read from it.

use std::fmt;

/// Where an action applies. This is the action's "when" clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Available anywhere in a project window.
    Window,
    /// Needs a focused editor.
    Editor,
}

/// Every command DumbJ can run.
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
}

impl Action {
    /// Every action, in the order menus and documentation list them.
    pub const ALL: &[Self] = &[
        Self::OpenFolder,
        Self::CloseTab,
        Self::NextTab,
        Self::PreviousTab,
        Self::ActivateProjectToolWindow,
        Self::HideAllToolWindows,
        Self::GotoLine,
        Self::Find,
        Self::FindNext,
        Self::FindPrevious,
        Self::ExtendSelection,
    ];

    /// The stable DumbJ action ID, used in `action-id-map.toml` and user configuration.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::OpenFolder => "project.open-folder",
            Self::CloseTab => "tabs.close",
            Self::NextTab => "tabs.next",
            Self::PreviousTab => "tabs.previous",
            Self::ActivateProjectToolWindow => "tool-window.project",
            Self::HideAllToolWindows => "tool-window.hide-all",
            Self::GotoLine => "editor.goto-line",
            Self::Find => "editor.find",
            Self::FindNext => "editor.find-next",
            Self::FindPrevious => "editor.find-previous",
            Self::ExtendSelection => "editor.extend-selection",
        }
    }

    /// The name shown to the user.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::OpenFolder => "Open Folder…",
            Self::CloseTab => "Close Tab",
            Self::NextTab => "Select Next Tab",
            Self::PreviousTab => "Select Previous Tab",
            Self::ActivateProjectToolWindow => "Project Tool Window",
            Self::HideAllToolWindows => "Hide All Tool Windows",
            Self::GotoLine => "Go to Line:Column…",
            Self::Find => "Find…",
            Self::FindNext => "Find Next",
            Self::FindPrevious => "Find Previous",
            Self::ExtendSelection => "Extend Selection",
        }
    }

    #[must_use]
    pub fn scope(self) -> Scope {
        match self {
            Self::OpenFolder
            | Self::CloseTab
            | Self::NextTab
            | Self::PreviousTab
            | Self::ActivateProjectToolWindow
            | Self::HideAllToolWindows => Scope::Window,
            Self::GotoLine
            | Self::Find
            | Self::FindNext
            | Self::FindPrevious
            | Self::ExtendSelection => Scope::Editor,
        }
    }

    /// Looks an action up by its DumbJ ID.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| action.id() == id)
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
    fn ids_are_unique_and_round_trip() {
        let ids: BTreeSet<_> = Action::ALL.iter().map(|action| action.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len());
        for &action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
            assert_ne!(action.title(), "");
        }
        assert_eq!(Action::from_id("nope"), None);
    }
}
