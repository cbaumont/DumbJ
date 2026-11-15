//! Checks the vendored keymaps end to end: known bindings, the action ID map and
//! `docs/KEYMAP.md`.

#![expect(
    clippy::unwrap_used,
    reason = "integration test: a panic is the failure report"
)]

use std::path::PathBuf;

use dumbj_actions::builtin::{self, Platform};
use dumbj_actions::{
    Action, Bindings, Dispatch, KeyDispatcher, Modifier, Modifiers, Shortcut, ShortcutStyle,
    render_keymap_markdown,
};

fn shown(keymap: &str, intellij_id: &str, style: ShortcutStyle) -> Vec<String> {
    let resolved = builtin::library().unwrap().resolve(keymap).unwrap();
    resolved
        .shortcuts(intellij_id)
        .iter()
        .filter_map(|s| s.display(style))
        .collect()
}

fn linux(intellij_id: &str) -> Vec<String> {
    shown(builtin::MACOS_FOR_ALL, intellij_id, ShortcutStyle::Linux)
}

fn macos(intellij_id: &str) -> Vec<String> {
    shown(builtin::IDEA_MACOS, intellij_id, ShortcutStyle::MacOs)
}

/// The canary bindings from BRIEF §6.1. If the vendored XML changes one of these, the XML
/// is right: update the test and tell the project owner.
#[test]
fn macos_for_all_known_bindings() {
    assert_eq!(linux("SearchEverywhere"), ["Shift Shift"]);
    assert_eq!(linux("GotoDeclaration"), ["Ctrl+B", "Ctrl+Click"]);
    assert_eq!(linux("FindUsages"), ["Alt+F7"]);
    assert_eq!(linux("FileStructurePopup"), ["Ctrl+F12"]);
    assert_eq!(linux("ActivateProjectToolWindow"), ["Alt+1"]);
    // BRIEF §6.1 and the plugin README say Ctrl+Alt+G / Alt+Shift+G. The vendored XML
    // lists FindNext twice (F3, then Ctrl+G) and IntelliJ keeps the later entry.
    assert_eq!(linux("FindNext"), ["Ctrl+G"]);
    assert_eq!(linux("FindPrevious"), ["Alt+Shift+G"]);
}

#[test]
fn macos_for_all_m0_actions() {
    assert_eq!(linux("CloseContent"), ["Ctrl+W"]);
    assert_eq!(linux("NextTab"), ["Ctrl+Shift+]"]);
    assert_eq!(linux("PreviousTab"), ["Ctrl+Shift+["]);
    assert_eq!(linux("HideAllWindows"), ["Ctrl+Shift+F12"]);
    assert_eq!(linux("GotoLine"), ["Ctrl+L"]);
    assert_eq!(linux("Find"), ["Ctrl+F"]);
    assert_eq!(linux("EditorSelectWord"), ["Alt+Up"]);
}

#[test]
fn intellij_macos_known_bindings() {
    assert_eq!(macos("SearchEverywhere"), ["Shift Shift"]);
    assert_eq!(macos("GotoDeclaration"), ["⌘B", "⌘Click", "Middle Click"]);
    assert_eq!(macos("CloseContent"), ["⌘W"]);
    assert_eq!(macos("ActivateProjectToolWindow"), ["⌘1"]);
    assert_eq!(macos("FindNext"), ["⌘G"]);
    // Inherited from $default with Control and Meta swapped, as IntelliJ does on macOS.
    assert_eq!(macos("HideAllWindows"), ["⇧⌘F12"]);
    assert_eq!(macos("FileStructurePopup"), ["⌘F12"]);
    assert_eq!(macos("FindUsages"), ["⌥F7"]);
}

#[test]
fn every_vendored_keymap_parses_cleanly() {
    let library = builtin::library().unwrap();
    for name in [
        builtin::IDEA_DEFAULT,
        builtin::IDEA_MACOS,
        builtin::MACOS_FOR_ALL,
    ] {
        let file = library.get(name).unwrap();
        assert_eq!(file.warnings(), [] as [String; 0], "warnings in `{name}`");
        library.resolve(name).unwrap();
    }
}

#[test]
fn every_action_is_mapped_from_an_intellij_id() {
    let map = builtin::action_id_map().unwrap();
    for action in Action::all() {
        assert!(
            map.intellij_ids(action).next().is_some(),
            "{action} has no IntelliJ ID"
        );
    }
}

#[test]
fn every_m0_action_except_open_folder_is_bound_on_both_platforms() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let bindings = builtin::default_bindings(platform).unwrap();
        for action in Action::all() {
            let bound = bindings
                .shortcuts(action)
                .iter()
                .any(|shortcut| !matches!(shortcut, Shortcut::Unsupported(_)));
            assert_eq!(
                bound,
                action != Action::OpenFolder,
                "{action} on {platform:?}"
            );
        }
        assert_ne!(bindings.unmapped_intellij_actions(), [] as [String; 0]);
    }
}

#[test]
fn default_linux_bindings_dispatch() {
    let bindings: Bindings = builtin::default_bindings(Platform::Linux).unwrap();
    let mut dispatcher = KeyDispatcher::new(&bindings);
    let stroke = |text: &str| text.parse().unwrap();
    assert_eq!(
        dispatcher.key_down(stroke("control W"), |_| true),
        Dispatch::Action(Action::CloseTab)
    );
    assert_eq!(dispatcher.key_down(stroke("alt 1"), |_| true), {
        Dispatch::Action(Action::ActivateProjectToolWindow)
    });
    assert_eq!(
        dispatcher.key_down(stroke("W"), |_| true),
        Dispatch::Unhandled
    );
    let click = |text: &str| text.parse::<dumbj_actions::MouseStroke>().unwrap();
    let all = |_| true;
    assert_eq!(
        dispatcher.mouse_down(click("ctrl button1"), all),
        Some(Action::GotoDeclaration)
    );
    assert_eq!(
        dispatcher.mouse_down(click("button4"), all),
        Some(Action::Back)
    );
    assert_eq!(
        dispatcher.mouse_down(click("button5"), all),
        Some(Action::Forward)
    );
    assert_eq!(
        dispatcher.mouse_down(click("button1"), all),
        None,
        "a plain click is unbound"
    );
    assert_eq!(
        dispatcher.mouse_down(click("ctrl button1"), |_| false),
        None
    );
    // Nothing in M0 is bound to double Shift, so it must not fire.
    let now = std::time::Instant::now();
    let shift = Modifiers::only(Modifier::Shift);
    for modifiers in [shift, Modifiers::NONE, shift, Modifiers::NONE] {
        assert_eq!(dispatcher.modifiers_changed(modifiers, now, |_| true), None);
    }
}

/// `docs/KEYMAP.md` is generated. Regenerate it with
/// `DUMBJ_REGENERATE_KEYMAP=1 cargo nextest run -p dumbj-actions keymap_md_is_up_to_date`.
#[test]
fn keymap_md_is_up_to_date() {
    let linux = builtin::default_bindings(Platform::Linux).unwrap();
    let macos = builtin::default_bindings(Platform::MacOs).unwrap();
    let expected = render_keymap_markdown(&linux, &macos, &builtin::action_id_map().unwrap());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/KEYMAP.md");
    if std::env::var_os("DUMBJ_REGENERATE_KEYMAP").is_some() {
        std::fs::write(&path, &expected).unwrap();
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "docs/KEYMAP.md is out of date. Regenerate it with \
         `DUMBJ_REGENERATE_KEYMAP=1 cargo nextest run -p dumbj-actions keymap_md_is_up_to_date`."
    );
}
