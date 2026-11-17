//! The semantics follow IntelliJ's `KeymapImpl.readExternal`: an `<action>` element replaces
//! every shortcut the parent chain defines for that action (an empty element clears them),
//! and when a file lists the same action twice the later entry wins. IntelliJ's built-in
//! macOS keymaps also swap Control and Meta in everything they inherit (`MacOSDefaultKeymap`).

use std::collections::BTreeMap;

use thiserror::Error;

use crate::keystroke::{KeyStroke, Shortcut, parse_double_modifier};

#[derive(Debug, Error)]
pub enum KeymapError {
    #[error("keymap XML is not well formed: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("expected a <keymap> root element, found <{0}>")]
    UnexpectedRoot(String),
    #[error("the <keymap> element has no `name` attribute")]
    MissingName,
    #[error("keymap `{keymap}` has an <action> element without an `id` attribute")]
    MissingActionId { keymap: String },
    #[error("keymap `{0}` is not loaded")]
    UnknownKeymap(String),
    #[error("keymap `{child}` names a parent `{parent}` that is not loaded")]
    UnknownParent { child: String, parent: String },
    #[error("keymap `{0}` inherits from itself")]
    ParentCycle(String),
}

#[derive(Debug, Clone)]
pub struct KeymapFile {
    name: String,
    parent: Option<String>,
    /// Action ID to shortcuts, in file order with later duplicates already applied.
    actions: BTreeMap<String, Vec<Shortcut>>,
    warnings: Vec<String>,
}

impl KeymapFile {
    /// Shortcuts that cannot be understood are skipped, as IntelliJ does, and reported
    /// through [`KeymapFile::warnings`].
    pub fn parse(xml: &str) -> Result<Self, KeymapError> {
        let document = roxmltree::Document::parse(xml)?;
        let root = document.root_element();
        if root.tag_name().name() != "keymap" {
            return Err(KeymapError::UnexpectedRoot(
                root.tag_name().name().to_owned(),
            ));
        }
        let name = root
            .attribute("name")
            .ok_or(KeymapError::MissingName)?
            .to_owned();
        let parent = root.attribute("parent").map(str::to_owned);

        let mut actions = BTreeMap::new();
        let mut warnings = Vec::new();
        for element in root.children().filter(roxmltree::Node::is_element) {
            if element.tag_name().name() != "action" {
                warnings.push(format!(
                    "ignored unknown element <{}>",
                    element.tag_name().name()
                ));
                continue;
            }
            let id = element
                .attribute("id")
                .ok_or_else(|| KeymapError::MissingActionId {
                    keymap: name.clone(),
                })?;
            let shortcuts = element
                .children()
                .filter(roxmltree::Node::is_element)
                .filter_map(|node| match parse_shortcut(node) {
                    Ok(shortcut) => Some(shortcut),
                    Err(problem) => {
                        warnings.push(format!("action `{id}`: {problem}"));
                        None
                    }
                })
                .collect();
            actions.insert(id.to_owned(), shortcuts);
        }
        Ok(Self {
            name,
            parent,
            actions,
            warnings,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

fn parse_shortcut(node: roxmltree::Node<'_, '_>) -> Result<Shortcut, String> {
    let attribute = |name: &str| {
        node.attribute(name)
            .ok_or_else(|| format!("<{}> has no `{name}` attribute", node.tag_name().name()))
    };
    let stroke = |text: &str| text.parse::<KeyStroke>().map_err(|error| error.to_string());
    match node.tag_name().name() {
        "keyboard-shortcut" => {
            let first = stroke(attribute("first-keystroke")?)?;
            let second = node.attribute("second-keystroke").map(stroke).transpose()?;
            Ok(Shortcut::Keyboard { first, second })
        }
        "keyboard-gesture-shortcut" => {
            let keystroke = attribute("keystroke")?;
            let modifier = attribute("modifier")?;
            let double = modifier.eq_ignore_ascii_case("dblClick");
            Ok(match parse_double_modifier(keystroke).filter(|_| double) {
                Some(modifier) => Shortcut::DoubleModifier(modifier),
                None => Shortcut::Unsupported(format!("{modifier} {keystroke}")),
            })
        }
        "mouse-shortcut" => {
            let keystroke = attribute("keystroke")?;
            // Touch gestures ("Force touch") have no button; keep them for reports.
            Ok(keystroke.parse().map_or_else(
                |_| Shortcut::Unsupported(format!("mouse {keystroke}")),
                Shortcut::Mouse,
            ))
        }
        other => Err(format!("unknown shortcut element <{other}>")),
    }
}

/// IntelliJ's built-in keymaps with this name prefix are `MacOSDefaultKeymap`s: shortcuts
/// they inherit from their parent have Control and Meta swapped, so that `$default`'s
/// `control B` reads as ⌘B on a Mac.
const MACOS_KEYMAP_PREFIX: &str = "Mac OS X";

fn swap_ctrl_and_meta(shortcut: &mut Shortcut) {
    let swap = |stroke: &mut KeyStroke| {
        std::mem::swap(&mut stroke.modifiers.ctrl, &mut stroke.modifiers.meta);
    };
    match shortcut {
        Shortcut::Keyboard { first, second } => {
            swap(first);
            second.as_mut().map(swap);
        }
        Shortcut::Mouse(stroke) => {
            std::mem::swap(&mut stroke.modifiers.ctrl, &mut stroke.modifiers.meta);
        }
        Shortcut::DoubleModifier(_) | Shortcut::Unsupported(_) => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeymap {
    name: String,
    actions: BTreeMap<String, Vec<Shortcut>>,
}

impl ResolvedKeymap {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn shortcuts(&self, intellij_action_id: &str) -> &[Shortcut] {
        self.actions
            .get(intellij_action_id)
            .map_or(&[], Vec::as_slice)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[Shortcut])> {
        self.actions
            .iter()
            .map(|(id, shortcuts)| (id.as_str(), shortcuts.as_slice()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeymapLibrary {
    files: BTreeMap<String, KeymapFile>,
}

impl KeymapLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, file: KeymapFile) {
        self.files.insert(file.name.clone(), file);
    }

    pub fn add_xml(&mut self, xml: &str) -> Result<String, KeymapError> {
        let file = KeymapFile::parse(xml)?;
        let name = file.name.clone();
        self.add(file);
        Ok(name)
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&KeymapFile> {
        self.files.get(name)
    }

    pub fn resolve(&self, name: &str) -> Result<ResolvedKeymap, KeymapError> {
        let mut chain = Vec::new();
        let mut current = self
            .files
            .get(name)
            .ok_or_else(|| KeymapError::UnknownKeymap(name.to_owned()))?;
        loop {
            if chain
                .iter()
                .any(|seen: &&KeymapFile| seen.name == current.name)
            {
                return Err(KeymapError::ParentCycle(name.to_owned()));
            }
            chain.push(current);
            let Some(parent) = &current.parent else { break };
            current = self
                .files
                .get(parent)
                .ok_or_else(|| KeymapError::UnknownParent {
                    child: current.name.clone(),
                    parent: parent.clone(),
                })?;
        }

        let mut actions: BTreeMap<String, Vec<Shortcut>> = BTreeMap::new();
        for file in chain.into_iter().rev() {
            if file.name.starts_with(MACOS_KEYMAP_PREFIX) {
                actions.values_mut().flatten().for_each(swap_ctrl_and_meta);
            }
            for (id, shortcuts) in &file.actions {
                if shortcuts.is_empty() {
                    actions.remove(id);
                } else {
                    actions.insert(id.clone(), shortcuts.clone());
                }
            }
        }
        Ok(ResolvedKeymap {
            name: name.to_owned(),
            actions,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::keystroke::Modifier;

    fn key(text: &str) -> Shortcut {
        Shortcut::key(text.parse().unwrap())
    }

    const BASE: &str = r#"
        <keymap name="base" version="1">
          <action id="Open"><keyboard-shortcut first-keystroke="control O"/></action>
          <action id="Close">
            <keyboard-shortcut first-keystroke="control F4"/>
            <mouse-shortcut keystroke="button2"/>
          </action>
          <action id="Search">
            <keyboard-gesture-shortcut keystroke="shift SHIFT" modifier="dblClick"/>
          </action>
          <action id="Chord">
            <keyboard-shortcut first-keystroke="control K" second-keystroke="control C"/>
          </action>
        </keymap>"#;

    const CHILD: &str = r#"
        <keymap name="child" parent="base">
          <action id="Close"><keyboard-shortcut first-keystroke="control W"/></action>
          <action id="Open"/>
          <action id="Dup"><keyboard-shortcut first-keystroke="F3"/></action>
          <action id="Dup"><keyboard-shortcut first-keystroke="control G"/></action>
        </keymap>"#;

    fn library() -> KeymapLibrary {
        let mut library = KeymapLibrary::new();
        library.add_xml(BASE).unwrap();
        library.add_xml(CHILD).unwrap();
        library
    }

    #[test]
    fn parses_every_shortcut_kind() {
        let base = library().resolve("base").unwrap();
        assert_eq!(base.shortcuts("Open"), [key("control O")]);
        assert_eq!(
            base.shortcuts("Close"),
            [
                key("control F4"),
                Shortcut::Mouse("button2".parse().unwrap())
            ]
        );
        assert_eq!(
            base.shortcuts("Search"),
            [Shortcut::DoubleModifier(Modifier::Shift)]
        );
        assert_eq!(
            base.shortcuts("Chord"),
            [Shortcut::Keyboard {
                first: "control K".parse().unwrap(),
                second: Some("control C".parse().unwrap())
            }]
        );
        assert_eq!(base.shortcuts("Missing"), []);
    }

    #[test]
    fn child_replaces_clears_and_inherits() {
        let child = library().resolve("child").unwrap();
        assert_eq!(
            child.shortcuts("Close"),
            [key("control W")],
            "child replaces parent"
        );
        assert_eq!(
            child.shortcuts("Open"),
            [],
            "empty element clears the parent's shortcuts"
        );
        assert_eq!(
            child.shortcuts("Chord").len(),
            1,
            "untouched actions are inherited"
        );
        assert_eq!(
            child.shortcuts("Dup"),
            [key("control G")],
            "later duplicate wins"
        );
        assert!(child.iter().all(|(id, _)| id != "Open"));
    }

    #[test]
    fn macos_keymaps_swap_ctrl_and_meta_in_inherited_shortcuts() {
        let mut library = library();
        library
            .add_xml(
                r#"<keymap name="Mac OS X 10.5+" parent="base">
                     <action id="Own"><keyboard-shortcut first-keystroke="control O"/></action>
                   </keymap>"#,
            )
            .unwrap();
        library
            .add_xml(r#"<keymap name="mine" parent="Mac OS X 10.5+"/>"#)
            .unwrap();
        for name in ["Mac OS X 10.5+", "mine"] {
            let mac = library.resolve(name).unwrap();
            assert_eq!(mac.shortcuts("Open"), [key("meta O")], "inherited: swapped");
            assert_eq!(mac.shortcuts("Own"), [key("control O")], "own: kept");
            assert_eq!(
                mac.shortcuts("Chord"),
                [Shortcut::Keyboard {
                    first: "meta K".parse().unwrap(),
                    second: Some("meta C".parse().unwrap())
                }]
            );
            assert_eq!(
                mac.shortcuts("Search"),
                [Shortcut::DoubleModifier(Modifier::Shift)]
            );
        }
    }

    #[test]
    fn bad_shortcuts_are_skipped_with_a_warning() {
        let file = KeymapFile::parse(
            r#"<keymap name="k">
                 <action id="A">
                   <keyboard-shortcut first-keystroke="control NOPE"/>
                   <keyboard-shortcut first-keystroke="control A"/>
                 </action>
               </keymap>"#,
        )
        .unwrap();
        assert_eq!(file.warnings().len(), 1);
        assert!(file.warnings()[0].contains("NOPE"), "{:?}", file.warnings());
        let mut library = KeymapLibrary::new();
        library.add(file);
        assert_eq!(
            library.resolve("k").unwrap().shortcuts("A"),
            [key("control A")]
        );
    }

    #[test]
    fn reports_structural_errors() {
        assert!(matches!(
            KeymapFile::parse("<nope/>"),
            Err(KeymapError::UnexpectedRoot(_))
        ));
        assert!(matches!(
            KeymapFile::parse("<keymap/>"),
            Err(KeymapError::MissingName)
        ));
        assert!(matches!(
            KeymapFile::parse(r#"<keymap name="k"><action/></keymap>"#),
            Err(KeymapError::MissingActionId { .. })
        ));
        assert!(matches!(
            KeymapFile::parse("<keymap"),
            Err(KeymapError::Xml(_))
        ));

        let mut library = KeymapLibrary::new();
        library
            .add_xml(r#"<keymap name="orphan" parent="gone"/>"#)
            .unwrap();
        library.add_xml(r#"<keymap name="a" parent="b"/>"#).unwrap();
        library.add_xml(r#"<keymap name="b" parent="a"/>"#).unwrap();
        assert!(matches!(
            library.resolve("orphan"),
            Err(KeymapError::UnknownParent { .. })
        ));
        assert!(matches!(
            library.resolve("a"),
            Err(KeymapError::ParentCycle(_))
        ));
        assert!(matches!(
            library.resolve("zzz"),
            Err(KeymapError::UnknownKeymap(_))
        ));
    }

    proptest! {
        #[test]
        fn parsing_never_panics(xml in "\\PC*") {
            let _ = KeymapFile::parse(&xml);
        }

        #[test]
        fn parsing_keymap_shaped_input_never_panics(
            id in "[a-zA-Z.$]{0,8}",
            first in "[a-zA-Z0-9_ ]{0,20}",
            second in "[a-zA-Z0-9_ ]{0,20}",
        ) {
            let xml = format!(
                r#"<keymap name="k"><action id="{id}">
                   <keyboard-shortcut first-keystroke="{first}" second-keystroke="{second}"/>
                   <keyboard-gesture-shortcut keystroke="{first}" modifier="{second}"/>
                   </action></keymap>"#
            );
            let _ = KeymapFile::parse(&xml);
        }
    }
}
