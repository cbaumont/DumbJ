use std::fmt::Write as _;

use crate::bindings::{ActionIdMap, Bindings};
use crate::keystroke::ShortcutStyle;
use crate::registry::Action;

fn shortcuts_cell(bindings: &Bindings, action: Action, style: ShortcutStyle) -> String {
    let shown: Vec<String> = bindings
        .shortcuts(action)
        .iter()
        .filter_map(|shortcut| shortcut.display(style))
        .map(|text| format!("`{}`", text.replace('`', "Backquote").replace('|', "Pipe")))
        .collect();
    if shown.is_empty() {
        "—".to_owned()
    } else {
        shown.join(" / ")
    }
}

#[must_use]
pub fn render_keymap_markdown(linux: &Bindings, macos: &Bindings, map: &ActionIdMap) -> String {
    let mut out = String::from(
        "# DumbJ keymap\n\n\
         <!-- Generated from keymaps/ by dumbj-actions. Do not edit by hand. -->\n\n",
    );
    let _ = writeln!(
        out,
        "Linux uses the `{}` keymap (IntelliJ's `macOS For All` plus DumbJ's own actions) and \
         macOS uses `{}` (IntelliJ's `Mac OS X 10.5+` plus the same actions).\n",
        linux.keymap_name(),
        macos.keymap_name()
    );
    out.push_str("| Action | DumbJ ID | IntelliJ ID | Linux | macOS |\n|---|---|---|---|---|\n");
    for action in Action::all() {
        let intellij: Vec<String> = map
            .intellij_ids(action)
            .map(|id| format!("`{id}`"))
            .collect();
        let _ = writeln!(
            out,
            "| {} | `{}` | {} | {} | {} |",
            action.title(),
            action.id(),
            intellij.join(", "),
            shortcuts_cell(linux, action, ShortcutStyle::Linux),
            shortcuts_cell(macos, action, ShortcutStyle::MacOs),
        );
    }
    out
}
