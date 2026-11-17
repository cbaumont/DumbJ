use std::collections::{BTreeMap, HashMap};

use dumbj_syntax::HighlightKind;
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    fn from_hex(hex: &str) -> Option<Self> {
        let digits = hex.strip_prefix('#').filter(|digits| digits.len() == 6)?;
        let channel = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
        Some(Self {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
            a: 0xff,
        })
    }

    fn with_alpha(self, alpha: f32) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "alpha is validated to be within 0..=1, so the product is within 0..=255"
        )]
        let a = (alpha * 255.0).round() as u8;
        Self { a, ..self }
    }

    #[must_use]
    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes([self.r, self.g, self.b, self.a])
    }

    #[must_use]
    pub fn luminance(self) -> f64 {
        let linear = |channel: u8| {
            let c = f64::from(channel) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.r) + 0.7152 * linear(self.g) + 0.0722 * linear(self.b)
    }

    #[must_use]
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiRole {
    Background,
    PanelBackground,
    ChromeBackground,
    PopupBackground,
    InputBackground,
    Border,
    PopupBorder,
    Text,
    TextMuted,
    TextFaint,
    Accent,
    Hover,
    Selected,
    SelectedInactive,
    FolderIcon,
    FileIcon,
    Error,
    Warning,
    Success,
    Info,
    EditorLineNumber,
    EditorLineNumberActive,
    EditorCurrentLine,
    EditorSelection,
    EditorCaret,
    EditorSearchMatch,
    EditorSearchMatchCurrent,
    EditorUsageRead,
    EditorUsageWrite,
}

impl UiRole {
    pub const ALL: &[Self] = &[
        Self::Background,
        Self::PanelBackground,
        Self::ChromeBackground,
        Self::PopupBackground,
        Self::InputBackground,
        Self::Border,
        Self::PopupBorder,
        Self::Text,
        Self::TextMuted,
        Self::TextFaint,
        Self::Accent,
        Self::Hover,
        Self::Selected,
        Self::SelectedInactive,
        Self::FolderIcon,
        Self::FileIcon,
        Self::Error,
        Self::Warning,
        Self::Success,
        Self::Info,
        Self::EditorLineNumber,
        Self::EditorLineNumberActive,
        Self::EditorCurrentLine,
        Self::EditorSelection,
        Self::EditorCaret,
        Self::EditorSearchMatch,
        Self::EditorSearchMatchCurrent,
        Self::EditorUsageRead,
        Self::EditorUsageWrite,
    ];

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::PanelBackground => "panel-background",
            Self::ChromeBackground => "chrome-background",
            Self::PopupBackground => "popup-background",
            Self::InputBackground => "input-background",
            Self::Border => "border",
            Self::PopupBorder => "popup-border",
            Self::Text => "text",
            Self::TextMuted => "text-muted",
            Self::TextFaint => "text-faint",
            Self::Accent => "accent",
            Self::Hover => "hover",
            Self::Selected => "selected",
            Self::SelectedInactive => "selected-inactive",
            Self::FolderIcon => "folder-icon",
            Self::FileIcon => "file-icon",
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Success => "success",
            Self::Info => "info",
            Self::EditorLineNumber => "editor-line-number",
            Self::EditorLineNumberActive => "editor-line-number-active",
            Self::EditorCurrentLine => "editor-current-line",
            Self::EditorSelection => "editor-selection",
            Self::EditorCaret => "editor-caret",
            Self::EditorSearchMatch => "editor-search-match",
            Self::EditorSearchMatchCurrent => "editor-search-match-current",
            Self::EditorUsageRead => "editor-usage-read",
            Self::EditorUsageWrite => "editor-usage-write",
        }
    }
}

#[must_use]
pub fn syntax_key(kind: HighlightKind) -> &'static str {
    match kind {
        HighlightKind::Comment => "comment",
        HighlightKind::Keyword => "keyword",
        HighlightKind::String => "string",
        HighlightKind::StringEscape => "string-escape",
        HighlightKind::Regex => "regex",
        HighlightKind::Number => "number",
        HighlightKind::ConstantBuiltin => "constant-builtin",
        HighlightKind::Constant => "constant",
        HighlightKind::Type => "type",
        HighlightKind::TypeBuiltin => "type-builtin",
        HighlightKind::Constructor => "constructor",
        HighlightKind::Function => "function",
        HighlightKind::FunctionBuiltin => "function-builtin",
        HighlightKind::Variable => "variable",
        HighlightKind::VariableBuiltin => "variable-builtin",
        HighlightKind::Parameter => "parameter",
        HighlightKind::Property => "property",
        HighlightKind::Attribute => "attribute",
        HighlightKind::Namespace => "namespace",
        HighlightKind::Label => "label",
        HighlightKind::Operator => "operator",
        HighlightKind::PunctuationBracket => "punctuation-bracket",
        HighlightKind::PunctuationDelimiter => "punctuation-delimiter",
        HighlightKind::PunctuationSpecial => "punctuation-special",
    }
}

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("theme file is not valid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("palette colour `{name}` has the value `{value}`, which is not #rrggbb")]
    BadColor { name: String, value: String },
    #[error("role `{role}` uses `{color}`, which the palette does not define")]
    UnknownPaletteColor { role: String, color: String },
    #[error("role `{role}` has alpha {alpha}, which is outside 0 to 1")]
    BadAlpha { role: String, alpha: f32 },
    #[error("the roles file does not define `{0}`")]
    MissingRole(String),
    #[error("the roles file defines `{0}`, which is not a known role")]
    UnknownRole(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    name: String,
    dark: bool,
    colors: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RoleValue {
    Name(String),
    Translucent { color: String, alpha: f32 },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RolesFile {
    ui: BTreeMap<String, RoleValue>,
    syntax: BTreeMap<String, RoleValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    name: String,
    dark: bool,
    ui: HashMap<UiRole, Rgba>,
    syntax: HashMap<HighlightKind, Rgba>,
}

fn resolve<R: Copy + Eq + std::hash::Hash>(
    table: &BTreeMap<String, RoleValue>,
    palette: &HashMap<&str, Rgba>,
    keys: impl Iterator<Item = (R, &'static str)>,
) -> Result<HashMap<R, Rgba>, ThemeError> {
    let lookup = |role: &str, color: &str| {
        palette
            .get(color)
            .copied()
            .ok_or_else(|| ThemeError::UnknownPaletteColor {
                role: role.to_owned(),
                color: color.to_owned(),
            })
    };
    let mut resolved = HashMap::new();
    let mut known = Vec::new();
    for (role, key) in keys {
        known.push(key);
        let value = table
            .get(key)
            .ok_or_else(|| ThemeError::MissingRole(key.to_owned()))?;
        let color = match value {
            RoleValue::Name(color) => lookup(key, color)?,
            RoleValue::Translucent { color, alpha } => {
                if !(0.0..=1.0).contains(alpha) {
                    return Err(ThemeError::BadAlpha {
                        role: key.to_owned(),
                        alpha: *alpha,
                    });
                }
                lookup(key, color)?.with_alpha(*alpha)
            }
        };
        resolved.insert(role, color);
    }
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(unknown) => Err(ThemeError::UnknownRole(unknown.clone())),
        None => Ok(resolved),
    }
}

impl Theme {
    pub fn load(palette_toml: &str, roles_toml: &str) -> Result<Self, ThemeError> {
        let palette_file: PaletteFile = toml::from_str(palette_toml)?;
        let roles: RolesFile = toml::from_str(roles_toml)?;
        let palette = palette_file
            .colors
            .iter()
            .map(|(name, value)| match Rgba::from_hex(value) {
                Some(color) => Ok((name.as_str(), color)),
                None => Err(ThemeError::BadColor {
                    name: name.clone(),
                    value: value.clone(),
                }),
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let ui = resolve(
            &roles.ui,
            &palette,
            UiRole::ALL.iter().map(|role| (*role, role.key())),
        )?;
        let syntax = resolve(
            &roles.syntax,
            &palette,
            HighlightKind::ALL
                .iter()
                .map(|kind| (*kind, syntax_key(*kind))),
        )?;
        Ok(Self {
            name: palette_file.name,
            dark: palette_file.dark,
            ui,
            syntax,
        })
    }

    pub fn catppuccin_mocha() -> Result<Self, ThemeError> {
        Self::load(
            include_str!("../../../themes/catppuccin/mocha.toml"),
            include_str!("../../../themes/catppuccin/roles.toml"),
        )
    }

    pub fn catppuccin_latte() -> Result<Self, ThemeError> {
        Self::load(
            include_str!("../../../themes/catppuccin/latte.toml"),
            include_str!("../../../themes/catppuccin/roles.toml"),
        )
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    #[must_use]
    pub fn ui(&self, role: UiRole) -> Rgba {
        // `load` guarantees every role is present; the fallback is unreachable.
        self.ui.get(&role).copied().unwrap_or(Rgba {
            r: 0xff,
            g: 0,
            b: 0xff,
            a: 0xff,
        })
    }

    #[must_use]
    pub fn syntax(&self, kind: HighlightKind) -> Rgba {
        self.syntax
            .get(&kind)
            .copied()
            .unwrap_or_else(|| self.ui(UiRole::Text))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const PALETTE: &str =
        "name = \"P\"\ndark = true\n[colors]\nink = \"#102030\"\npaper = \"#ffffff\"\n";

    fn roles(ui_extra: &str, replace_first: Option<&str>) -> String {
        let mut out = String::from("[ui]\n");
        for (index, role) in UiRole::ALL.iter().enumerate() {
            match (index, replace_first) {
                (0, Some(line)) => out.push_str(line),
                _ => writeln!(out, "{} = \"ink\"", role.key()).unwrap(),
            }
        }
        out.push_str(ui_extra);
        out.push_str("[syntax]\n");
        for kind in HighlightKind::ALL {
            writeln!(out, "{} = \"paper\"", syntax_key(*kind)).unwrap();
        }
        out
    }

    #[test]
    fn mocha_uses_the_exact_catppuccin_values() {
        let theme = Theme::catppuccin_mocha().unwrap();
        assert_eq!(theme.name(), "Catppuccin Mocha");
        assert!(theme.is_dark());
        assert_eq!(theme.ui(UiRole::Background).to_u32(), 0x1e1e_2eff);
        assert_eq!(theme.ui(UiRole::PanelBackground).to_u32(), 0x1818_25ff);
        assert_eq!(theme.ui(UiRole::ChromeBackground).to_u32(), 0x1111_1bff);
        assert_eq!(theme.ui(UiRole::Text).to_u32(), 0xcdd6_f4ff);
        assert_eq!(theme.ui(UiRole::Accent).to_u32(), 0xcba6_f7ff);
        assert_eq!(theme.ui(UiRole::EditorCaret).to_u32(), 0xf5e0_dcff);
        assert_eq!(theme.syntax(HighlightKind::Keyword).to_u32(), 0xcba6_f7ff);
        assert_eq!(theme.syntax(HighlightKind::String).to_u32(), 0xa6e3_a1ff);
        assert_eq!(theme.syntax(HighlightKind::Comment).to_u32(), 0x9399_b2ff);
        assert_eq!(
            theme.ui(UiRole::EditorSelection).to_u32(),
            0x9399_b24d,
            "30% alpha"
        );
    }

    #[test]
    fn latte_loads_and_is_light() {
        let theme = Theme::catppuccin_latte().unwrap();
        assert!(!theme.is_dark());
        assert_eq!(theme.ui(UiRole::Background).to_u32(), 0xeff1_f5ff);
        assert_eq!(theme.syntax(HighlightKind::Keyword).to_u32(), 0x8839_efff);
    }

    #[test]
    fn text_is_readable_in_both_flavours() {
        for theme in [
            Theme::catppuccin_mocha().unwrap(),
            Theme::catppuccin_latte().unwrap(),
        ] {
            for background in [
                UiRole::Background,
                UiRole::PanelBackground,
                UiRole::PopupBackground,
            ] {
                let background = theme.ui(background);
                let contrast = theme.ui(UiRole::Text).contrast(background);
                assert!(
                    contrast >= 4.5,
                    "{}: text contrast {contrast:.1}",
                    theme.name()
                );
                let muted = theme.ui(UiRole::TextMuted).contrast(background);
                assert!(
                    muted >= 4.5,
                    "{}: muted text contrast {muted:.1}",
                    theme.name()
                );
            }
            // Latte's accent colours are low-contrast by upstream design (yellow is 2.2:1), and
            // the palette must be used exactly, so only Mocha is held to AA for code.
            let minimum = if theme.is_dark() { 4.5 } else { 2.0 };
            for kind in HighlightKind::ALL {
                let contrast = theme.syntax(*kind).contrast(theme.ui(UiRole::Background));
                assert!(
                    contrast >= minimum,
                    "{}: {kind:?} contrast {contrast:.1}",
                    theme.name()
                );
            }
        }
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let black = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let white = Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        assert!((black.contrast(white) - 21.0).abs() < 1e-9);
        assert!((white.contrast(white) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn loads_a_custom_theme_with_alpha() {
        let roles = roles(
            "",
            Some("background = { color = \"paper\", alpha = 0.5 }\n"),
        );
        let theme = Theme::load(PALETTE, &roles).unwrap();
        assert_eq!(
            theme.ui(UiRole::Background),
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 128
            }
        );
        assert_eq!(theme.ui(UiRole::Text).to_u32(), 0x1020_30ff);
        assert_eq!(theme.syntax(HighlightKind::Keyword).to_u32(), 0xffff_ffff);
    }

    #[test]
    fn rejects_incomplete_or_wrong_files() {
        let load = |palette: &str, roles: &str| Theme::load(palette, roles).unwrap_err();
        assert!(matches!(
            load("nope", &roles("", None)),
            ThemeError::Toml(_)
        ));
        assert!(matches!(
            load(&PALETTE.replace("#102030", "#12345"), &roles("", None)),
            ThemeError::BadColor { .. }
        ));
        assert!(matches!(
            load(PALETTE, &roles("", Some(""))),
            ThemeError::MissingRole(role) if role == "background"
        ));
        assert!(matches!(
            load(PALETTE, &roles("bogus = \"ink\"\n", None)),
            ThemeError::UnknownRole(role) if role == "bogus"
        ));
        assert!(matches!(
            load(PALETTE, &roles("", Some("background = \"mud\"\n"))),
            ThemeError::UnknownPaletteColor { .. }
        ));
        assert!(matches!(
            load(
                PALETTE,
                &roles("", Some("background = { color = \"ink\", alpha = 1.5 }\n"))
            ),
            ThemeError::BadAlpha { .. }
        ));
    }

    #[test]
    fn role_keys_are_unique() {
        let mut keys: Vec<_> = UiRole::ALL.iter().map(|role| role.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), UiRole::ALL.len());
        let mut keys: Vec<_> = HighlightKind::ALL
            .iter()
            .map(|kind| syntax_key(*kind))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), HighlightKind::ALL.len());
    }
}
