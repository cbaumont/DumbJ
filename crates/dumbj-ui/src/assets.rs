use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../../../assets/icons/lucide/", $name, ".svg")).as_slice(),
        )),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "chevron-down",
    "chevron-right",
    "file",
    "file-code",
    "folder",
    "folder-open",
    "folder-tree",
    "git-compare",
    "git-pull-request",
    "minus",
    "search",
    "square",
    "x",
];

pub(crate) fn fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/jetbrains-mono/JetBrainsMono-Bold.ttf"
        )),
        Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/jetbrains-mono/JetBrainsMono-Italic.ttf"
        )),
    ]
}

pub(crate) const EDITOR_FONT: &str = "JetBrains Mono";

pub(crate) struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_embedded_icons() {
        let folder = Assets.load("icons/folder.svg").unwrap().unwrap();
        assert!(folder.starts_with(b"<svg"));
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }
}
