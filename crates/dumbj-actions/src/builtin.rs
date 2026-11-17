use thiserror::Error;

use crate::bindings::{ActionIdMap, ActionMapError, Bindings};
use crate::keymap::{KeymapError, KeymapLibrary};
use crate::keystroke::ShortcutStyle;

const IDEA_DEFAULT_XML: &str = include_str!("../../../keymaps/idea-default.xml");
const IDEA_MACOS_XML: &str = include_str!("../../../keymaps/idea-macos.xml");
const MACOS_FOR_ALL_XML: &str = include_str!("../../../keymaps/macos-for-all.xml");
const DUMBJ_LINUX_XML: &str = include_str!("../../../keymaps/dumbj-linux.xml");
const DUMBJ_MACOS_XML: &str = include_str!("../../../keymaps/dumbj-macos.xml");
const ACTION_ID_MAP_TOML: &str = include_str!("../../../keymaps/action-id-map.toml");

/// The default keymap on Linux: "macOS For All" plus the shortcuts of DumbJ's own actions.
pub const DUMBJ_LINUX: &str = "DumbJ";
/// The default keymap on macOS: IntelliJ's "macOS" keymap plus DumbJ's own actions.
pub const DUMBJ_MACOS: &str = "DumbJ macOS";

#[derive(Debug, Error)]
pub enum BuiltinError {
    #[error(transparent)]
    Keymap(#[from] KeymapError),
    #[error(transparent)]
    ActionMap(#[from] ActionMapError),
}

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

    #[must_use]
    pub fn default_keymap(self) -> &'static str {
        match self {
            Self::Linux => DUMBJ_LINUX,
            Self::MacOs => DUMBJ_MACOS,
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

pub fn library() -> Result<KeymapLibrary, KeymapError> {
    let mut library = KeymapLibrary::new();
    for xml in [
        IDEA_DEFAULT_XML,
        IDEA_MACOS_XML,
        MACOS_FOR_ALL_XML,
        DUMBJ_LINUX_XML,
        DUMBJ_MACOS_XML,
    ] {
        library.add_xml(xml)?;
    }
    Ok(library)
}

pub fn action_id_map() -> Result<ActionIdMap, ActionMapError> {
    ActionIdMap::parse(ACTION_ID_MAP_TOML)
}

pub fn default_bindings(platform: Platform) -> Result<Bindings, BuiltinError> {
    let keymap = library()?.resolve(platform.default_keymap())?;
    Ok(Bindings::new(&keymap, &action_id_map()?))
}
