//! DumbJ's actions and keymaps.
//!
//! - [`Action`] is the registry of commands.
//! - [`KeymapLibrary`] loads IntelliJ-format keymap XML and resolves `parent` inheritance.
//! - [`ActionIdMap`] and [`Bindings`] map IntelliJ action IDs onto DumbJ actions.
//! - [`KeyDispatcher`] matches key events (strokes, chords, double Shift) against bindings.
//! - [`builtin`] holds the vendored keymaps: macOS For All (Linux default) and IntelliJ macOS.

mod bindings;
pub mod builtin;
mod dispatch;
mod keymap;
mod keymap_doc;
mod keystroke;
mod registry;

pub use bindings::{ActionIdMap, ActionMapError, Bindings};
pub use dispatch::{Dispatch, KeyDispatcher};
pub use keymap::{KeymapError, KeymapFile, KeymapLibrary, ResolvedKeymap};
pub use keymap_doc::render_keymap_markdown;
pub use keystroke::{
    Key, KeyStroke, KeyStrokeParseError, Modifier, Modifiers, Shortcut, ShortcutStyle,
};
pub use registry::{Action, Scope};
