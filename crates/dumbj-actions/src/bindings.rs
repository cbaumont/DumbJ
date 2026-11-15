use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

use crate::keymap::ResolvedKeymap;
use crate::keystroke::Shortcut;
use crate::registry::Action;

#[derive(Debug, Error)]
pub enum ActionMapError {
    #[error("action ID map is not valid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("action ID map sends `{intellij}` to `{dumbj}`, which is not a DumbJ action")]
    UnknownAction { intellij: String, dumbj: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionMapFile {
    actions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionIdMap {
    by_intellij_id: BTreeMap<String, Action>,
}

impl ActionIdMap {
    /// Parses the TOML table. Every right-hand side must be a registered DumbJ action ID.
    pub fn parse(toml_text: &str) -> Result<Self, ActionMapError> {
        let file: ActionMapFile = toml::from_str(toml_text)?;
        let by_intellij_id = file
            .actions
            .into_iter()
            .map(|(intellij, dumbj)| match Action::from_id(&dumbj) {
                Some(action) => Ok((intellij, action)),
                None => Err(ActionMapError::UnknownAction { intellij, dumbj }),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { by_intellij_id })
    }

    #[must_use]
    pub fn action(&self, intellij_id: &str) -> Option<Action> {
        self.by_intellij_id.get(intellij_id).copied()
    }

    /// The IntelliJ action IDs that map to `action`, sorted.
    pub fn intellij_ids(&self, action: Action) -> impl Iterator<Item = &str> {
        self.by_intellij_id
            .iter()
            .filter(move |(_, mapped)| **mapped == action)
            .map(|(id, _)| id.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    keymap_name: String,
    by_action: BTreeMap<Action, Vec<Shortcut>>,
    unmapped: Vec<String>,
}

impl Bindings {
    #[must_use]
    pub fn new(keymap: &ResolvedKeymap, map: &ActionIdMap) -> Self {
        let mut by_action: BTreeMap<Action, Vec<Shortcut>> = BTreeMap::new();
        let mut unmapped = Vec::new();
        for (intellij_id, shortcuts) in keymap.iter() {
            match map.action(intellij_id) {
                Some(action) => {
                    let bound = by_action.entry(action).or_default();
                    for shortcut in shortcuts {
                        if !bound.contains(shortcut) {
                            bound.push(shortcut.clone());
                        }
                    }
                }
                None => unmapped.push(intellij_id.to_owned()),
            }
        }
        Self {
            keymap_name: keymap.name().to_owned(),
            by_action,
            unmapped,
        }
    }

    #[must_use]
    pub fn keymap_name(&self) -> &str {
        &self.keymap_name
    }

    /// The shortcuts bound to `action`; empty if it is unbound.
    #[must_use]
    pub fn shortcuts(&self, action: Action) -> &[Shortcut] {
        self.by_action.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Action, &[Shortcut])> {
        self.by_action
            .iter()
            .map(|(action, shortcuts)| (*action, shortcuts.as_slice()))
    }

    /// Debug report: IntelliJ actions the keymap binds that DumbJ has no action for, sorted.
    #[must_use]
    pub fn unmapped_intellij_actions(&self) -> &[String] {
        &self.unmapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeymapLibrary;

    fn keymap() -> ResolvedKeymap {
        let mut library = KeymapLibrary::new();
        library
            .add_xml(
                r#"<keymap name="k">
                     <action id="CloseContent"><keyboard-shortcut first-keystroke="control W"/></action>
                     <action id="CloseActiveTab"><keyboard-shortcut first-keystroke="control W"/><keyboard-shortcut first-keystroke="control F4"/></action>
                     <action id="Unrelated"><keyboard-shortcut first-keystroke="F1"/></action>
                   </keymap>"#,
            )
            .unwrap();
        library.resolve("k").unwrap()
    }

    #[test]
    fn binds_mapped_actions_and_reports_the_rest() {
        let map = ActionIdMap::parse(
            "[actions]\nCloseContent = \"tabs.close\"\nCloseActiveTab = \"tabs.close\"\nGotoLine = \"editor.goto-line\"\n",
        )
        .unwrap();
        let bindings = Bindings::new(&keymap(), &map);
        let close: Vec<Shortcut> = ["control W", "control F4"]
            .iter()
            .map(|s| Shortcut::key(s.parse().unwrap()))
            .collect();
        assert_eq!(
            bindings.shortcuts(Action::CloseTab),
            close,
            "merged without duplicates"
        );
        assert_eq!(
            bindings.shortcuts(Action::GotoLine),
            [],
            "mapped but unbound in the keymap"
        );
        assert_eq!(bindings.unmapped_intellij_actions(), ["Unrelated"]);
        assert_eq!(
            map.intellij_ids(Action::CloseTab).collect::<Vec<_>>(),
            ["CloseActiveTab", "CloseContent"]
        );
    }

    #[test]
    fn rejects_unknown_dumbj_ids_and_bad_toml() {
        assert!(matches!(
            ActionIdMap::parse("[actions]\nFoo = \"no.such-action\"\n"),
            Err(ActionMapError::UnknownAction { .. })
        ));
        assert!(matches!(
            ActionIdMap::parse("actions = 3"),
            Err(ActionMapError::Toml(_))
        ));
        assert!(matches!(
            ActionIdMap::parse("[actions]\n[extra]\n"),
            Err(ActionMapError::Toml(_))
        ));
    }
}
