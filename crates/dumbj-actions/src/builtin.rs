//! The keymaps and action ID map that ship inside the binary, read from `keymaps/`.

use thiserror::Error;

use crate::bindings::{ActionIdMap, ActionMapError, Bindings};
use crate::keymap::{KeymapError, KeymapLibrary};
use crate::keystroke::ShortcutStyle;

const IDEA_DEFAULT_XML: &str = include_str!("../../../keymaps/idea-default.xml");
const IDEA_MACOS_XML: &str = include_str!("../../../keymaps/idea-macos.xml");
const MACOS_FOR_ALL_XML: &str = include_str!("../../../keymaps/macos-for-all.xml");
const ACTION_ID_MAP_TOML: &str = include_str!("../../../keymaps/action-id-map.toml");

/// The default keymap on Linux.
pub const MACOS_FOR_ALL: &str = "macOS For All";
/// The default keymap on macOS: IntelliJ's "macOS" keymap, under its internal name.
pub const IDEA_MACOS: &str = "Mac OS X 10.5+";
/// IntelliJ's root keymap, the parent of the other two.
pub const IDEA_DEFAULT: &str = "$default";

/// A problem with the embedded keymap data. Tests guarantee it does not happen in a release.
#[derive(Debug, Error)]
pub enum BuiltinError {
    #[error(transparent)]
    Keymap(#[from] KeymapError),
    #[error(transparent)]
    ActionMap(#[from] ActionMapError),
}

/// The platforms DumbJ has a default keymap for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

impl Platform {
    /// The platform this binary was compiled for. Anything that is not macOS counts as Linux.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The name of the platform's default keymap.
    #[must_use]
    pub fn default_keymap(self) -> &'static str {
        match self {
            Self::Linux => MACOS_FOR_ALL,
            Self::MacOs => IDEA_MACOS,
        }
    }

    #[must_use]
    pub fn shortcut_style(self) -> ShortcutStyle {
        match self {
            Self::Linux => ShortcutStyle::Linux,
            Self::MacOs => ShortcutStyle::MacOs,
        }
    }
}

/// A library holding every vendored keymap.
pub fn library() -> Result<KeymapLibrary, KeymapError> {
    let mut library = KeymapLibrary::new();
    for xml in [IDEA_DEFAULT_XML, IDEA_MACOS_XML, MACOS_FOR_ALL_XML] {
        library.add_xml(xml)?;
    }
    Ok(library)
}

/// The vendored IntelliJ-to-DumbJ action ID table.
pub fn action_id_map() -> Result<ActionIdMap, ActionMapError> {
    ActionIdMap::parse(ACTION_ID_MAP_TOML)
}

/// The default bindings for `platform`.
pub fn default_bindings(platform: Platform) -> Result<Bindings, BuiltinError> {
    let keymap = library()?.resolve(platform.default_keymap())?;
    Ok(Bindings::new(&keymap, &action_id_map()?))
}
