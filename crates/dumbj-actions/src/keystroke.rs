use std::fmt;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    /// ⌘ on macOS, Super on Linux.
    Meta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent modifier flags"
)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    #[must_use]
    pub fn only(modifier: Modifier) -> Self {
        let mut modifiers = Self::NONE;
        modifiers.insert(modifier);
        modifiers
    }

    pub fn insert(&mut self, modifier: Modifier) {
        match modifier {
            Modifier::Ctrl => self.ctrl = true,
            Modifier::Alt => self.alt = true,
            Modifier::Shift => self.shift = true,
            Modifier::Meta => self.meta = true,
        }
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Escape,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    ContextMenu,
    F(u8),
    Numpad(u8),
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
}

impl Key {
    fn from_awt(name: &str) -> Option<Self> {
        let upper = name.to_ascii_uppercase();
        let key = match upper.as_str() {
            "ENTER" => Self::Enter,
            "TAB" => Self::Tab,
            "SPACE" => Self::Space,
            "BACK_SPACE" => Self::Backspace,
            "DELETE" => Self::Delete,
            "ESCAPE" => Self::Escape,
            "INSERT" => Self::Insert,
            "HOME" => Self::Home,
            "END" => Self::End,
            "PAGE_UP" => Self::PageUp,
            "PAGE_DOWN" => Self::PageDown,
            "UP" | "KP_UP" => Self::Up,
            "DOWN" | "KP_DOWN" => Self::Down,
            "LEFT" | "KP_LEFT" => Self::Left,
            "RIGHT" | "KP_RIGHT" => Self::Right,
            "CONTEXT_MENU" => Self::ContextMenu,
            "ADD" => Self::NumpadAdd,
            "SUBTRACT" => Self::NumpadSubtract,
            "MULTIPLY" => Self::NumpadMultiply,
            "DIVIDE" => Self::NumpadDivide,
            "OPEN_BRACKET" => Self::Char('['),
            "CLOSE_BRACKET" => Self::Char(']'),
            "SLASH" => Self::Char('/'),
            "BACK_SLASH" => Self::Char('\\'),
            "BACK_QUOTE" => Self::Char('`'),
            "QUOTE" => Self::Char('\''),
            "COMMA" => Self::Char(','),
            "PERIOD" => Self::Char('.'),
            "MINUS" => Self::Char('-'),
            "EQUALS" => Self::Char('='),
            "PLUS" => Self::Char('+'),
            "SEMICOLON" => Self::Char(';'),
            other => return Self::from_awt_pattern(other),
        };
        Some(key)
    }

    fn from_awt_pattern(upper: &str) -> Option<Self> {
        let mut chars = upper.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return (c.is_ascii_uppercase() || c.is_ascii_digit())
                .then(|| Self::Char(c.to_ascii_lowercase()));
        }
        if let Some(digit) = upper.strip_prefix("NUMPAD") {
            return digit.parse().ok().filter(|n| *n <= 9).map(Self::Numpad);
        }
        if let Some(number) = upper.strip_prefix('F') {
            return number
                .parse()
                .ok()
                .filter(|n| (1..=24).contains(n))
                .map(Self::F);
        }
        None
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Char(c) => return write!(f, "{}", c.to_ascii_uppercase()),
            Self::F(n) => return write!(f, "F{n}"),
            Self::Numpad(n) => return write!(f, "Num{n}"),
            Self::Enter => "Enter",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Escape => "Esc",
            Self::Insert => "Insert",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PgUp",
            Self::PageDown => "PgDn",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::ContextMenu => "Menu",
            Self::NumpadAdd => "Num+",
            Self::NumpadSubtract => "Num-",
            Self::NumpadMultiply => "Num*",
            Self::NumpadDivide => "Num/",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutStyle {
    Linux,
    MacOs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyStroke {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl KeyStroke {
    #[must_use]
    pub fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    #[must_use]
    pub fn display(self, style: ShortcutStyle) -> String {
        let Modifiers {
            ctrl,
            alt,
            shift,
            meta,
        } = self.modifiers;
        let mut out = String::new();
        match style {
            ShortcutStyle::Linux => {
                for (held, name) in [
                    (ctrl, "Ctrl+"),
                    (alt, "Alt+"),
                    (shift, "Shift+"),
                    (meta, "Super+"),
                ] {
                    if held {
                        out.push_str(name);
                    }
                }
            }
            ShortcutStyle::MacOs => {
                // Apple's order: Control, Option, Shift, Command.
                for (held, glyph) in [(ctrl, '⌃'), (alt, '⌥'), (shift, '⇧'), (meta, '⌘')] {
                    if held {
                        out.push(glyph);
                    }
                }
            }
        }
        out.push_str(&self.key.to_string());
        out
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyStrokeParseError {
    #[error("key stroke is empty")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("key stroke `{0}` names only modifiers")]
    ModifierOnly(String),
}

fn parse_modifier(token: &str) -> Option<Modifier> {
    match token.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => Some(Modifier::Ctrl),
        "alt" => Some(Modifier::Alt),
        "shift" => Some(Modifier::Shift),
        "meta" => Some(Modifier::Meta),
        _ => None,
    }
}

fn split_awt(text: &str) -> Result<(Modifiers, &str), KeyStrokeParseError> {
    let mut tokens: Vec<&str> = text
        .split_ascii_whitespace()
        // AWT allows an explicit event type; only key presses are meaningful to us.
        .filter(|token| !token.eq_ignore_ascii_case("pressed"))
        .collect();
    let key = tokens.pop().ok_or(KeyStrokeParseError::Empty)?;
    let mut modifiers = Modifiers::NONE;
    for token in tokens {
        let modifier = parse_modifier(token)
            .ok_or_else(|| KeyStrokeParseError::UnknownKey(token.to_owned()))?;
        modifiers.insert(modifier);
    }
    Ok((modifiers, key))
}

impl FromStr for KeyStroke {
    type Err = KeyStrokeParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (modifiers, key) = split_awt(text)?;
        match Key::from_awt(key) {
            Some(key) => Ok(Self { modifiers, key }),
            None if is_modifier_key_name(key) => {
                Err(KeyStrokeParseError::ModifierOnly(text.to_owned()))
            }
            None => Err(KeyStrokeParseError::UnknownKey(key.to_owned())),
        }
    }
}

fn is_modifier_key_name(name: &str) -> bool {
    parse_modifier(name).is_some()
}

/// Parses the `keystroke` of a `dblClick` keyboard gesture whose key is itself a modifier,
/// e.g. `"shift SHIFT"` (double Shift). Returns `None` for any other gesture.
pub(crate) fn parse_double_modifier(text: &str) -> Option<Modifier> {
    let (modifiers, key) = split_awt(text).ok()?;
    let modifier = parse_modifier(key)?;
    (modifiers == Modifiers::only(modifier)).then_some(modifier)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MouseStroke {
    pub modifiers: Modifiers,
    /// The AWT button number: 1 left, 2 middle, 3 right, 4 back, 5 forward.
    pub button: u8,
    /// 1 for a click, 2 for a double click.
    pub clicks: u8,
}

impl MouseStroke {
    #[must_use]
    pub fn display(self, style: ShortcutStyle) -> String {
        let button = match (self.button, self.clicks) {
            (1, 1) => "Click".to_owned(),
            (1, _) => "Double Click".to_owned(),
            (2, _) => "Middle Click".to_owned(),
            (3, _) => "Right Click".to_owned(),
            (4, _) => "Mouse Back".to_owned(),
            (5, _) => "Mouse Forward".to_owned(),
            (other, _) => format!("Button {other}"),
        };
        let prefix = KeyStroke::new(self.modifiers, Key::Space).display(style);
        let prefix = prefix.trim_end_matches("Space");
        format!("{prefix}{button}")
    }
}

impl FromStr for MouseStroke {
    type Err = KeyStrokeParseError;

    /// Parses IntelliJ's mouse shortcut syntax: modifiers, `buttonN` and an optional
    /// `doubleClick`, in any order (`"ctrl button1"`, `"alt button1 doubleClick"`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut stroke = Self {
            modifiers: Modifiers::NONE,
            button: 0,
            clicks: 1,
        };
        for token in text.split_ascii_whitespace() {
            if let Some(modifier) = parse_modifier(token) {
                stroke.modifiers.insert(modifier);
            } else if token.eq_ignore_ascii_case("doubleClick") {
                stroke.clicks = 2;
            } else if let Some(button) = token.strip_prefix("button").and_then(|n| n.parse().ok()) {
                stroke.button = button;
            } else {
                return Err(KeyStrokeParseError::UnknownKey(token.to_owned()));
            }
        }
        if stroke.button == 0 {
            return Err(KeyStrokeParseError::Empty);
        }
        Ok(stroke)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Shortcut {
    Keyboard {
        first: KeyStroke,
        second: Option<KeyStroke>,
    },
    DoubleModifier(Modifier),
    Mouse(MouseStroke),
    /// A shortcut kind DumbJ does not dispatch (touch gestures and other key gestures).
    /// Holds the raw keymap text so reports can show it.
    Unsupported(String),
}

impl Shortcut {
    #[must_use]
    pub fn key(stroke: KeyStroke) -> Self {
        Self::Keyboard {
            first: stroke,
            second: None,
        }
    }

    #[must_use]
    pub fn display(&self, style: ShortcutStyle) -> Option<String> {
        match self {
            Self::Keyboard {
                first,
                second: None,
            } => Some(first.display(style)),
            Self::Keyboard {
                first,
                second: Some(second),
            } => Some(format!(
                "{}, {}",
                first.display(style),
                second.display(style)
            )),
            Self::DoubleModifier(modifier) => {
                let name = match (style, modifier) {
                    (_, Modifier::Shift) => "Shift",
                    (_, Modifier::Ctrl) => "Ctrl",
                    (ShortcutStyle::Linux, Modifier::Alt) => "Alt",
                    (ShortcutStyle::MacOs, Modifier::Alt) => "Option",
                    (ShortcutStyle::Linux, Modifier::Meta) => "Super",
                    (ShortcutStyle::MacOs, Modifier::Meta) => "Cmd",
                };
                Some(format!("{name} {name}"))
            }
            Self::Mouse(stroke) => Some(stroke.display(style)),
            Self::Unsupported(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn stroke(text: &str) -> KeyStroke {
        text.parse().unwrap()
    }

    #[test]
    fn parses_modifiers_and_named_keys() {
        assert_eq!(
            stroke("control shift OPEN_BRACKET"),
            KeyStroke::new(
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::NONE
                },
                Key::Char('[')
            )
        );
        assert_eq!(stroke("F3"), KeyStroke::new(Modifiers::NONE, Key::F(3)));
        assert_eq!(stroke("alt 1").key, Key::Char('1'));
        assert_eq!(stroke("ctrl alt NUMPAD5").key, Key::Numpad(5));
        assert_eq!(stroke("meta BACK_SPACE").key, Key::Backspace);
    }

    #[test]
    fn modifier_and_key_names_are_case_insensitive() {
        assert_eq!(stroke("Shift Enter"), stroke("shift ENTER"));
        assert_eq!(stroke("control f9"), stroke("control F9"));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("".parse::<KeyStroke>(), Err(KeyStrokeParseError::Empty));
        assert_eq!(
            "control NOPE".parse::<KeyStroke>(),
            Err(KeyStrokeParseError::UnknownKey("NOPE".into()))
        );
        assert_eq!(
            "hyper A".parse::<KeyStroke>(),
            Err(KeyStrokeParseError::UnknownKey("hyper".into()))
        );
        assert!(matches!(
            "shift SHIFT".parse::<KeyStroke>(),
            Err(KeyStrokeParseError::ModifierOnly(_))
        ));
        assert!("F25".parse::<KeyStroke>().is_err());
        assert!("NUMPAD10".parse::<KeyStroke>().is_err());
    }

    #[test]
    fn double_modifier_gestures() {
        assert_eq!(parse_double_modifier("shift SHIFT"), Some(Modifier::Shift));
        assert_eq!(parse_double_modifier("ctrl control"), Some(Modifier::Ctrl));
        assert_eq!(parse_double_modifier("control alt 1"), None);
        assert_eq!(parse_double_modifier("alt SHIFT"), None);
    }

    #[test]
    fn parses_and_displays_mouse_strokes() {
        let ctrl_click: MouseStroke = "ctrl button1".parse().unwrap();
        assert_eq!(ctrl_click.button, 1);
        assert!(ctrl_click.modifiers.ctrl);
        assert_eq!(ctrl_click.display(ShortcutStyle::Linux), "Ctrl+Click");
        let double: MouseStroke = "alt button1 doubleClick".parse().unwrap();
        assert_eq!((double.button, double.clicks), (1, 2));
        assert_eq!(double.display(ShortcutStyle::Linux), "Alt+Double Click");
        let back: MouseStroke = "button4".parse().unwrap();
        assert_eq!(back.display(ShortcutStyle::Linux), "Mouse Back");
        let meta: MouseStroke = "meta shift button1".parse().unwrap();
        assert_eq!(meta.display(ShortcutStyle::MacOs), "⇧⌘Click");
        assert!("Force touch".parse::<MouseStroke>().is_err());
        assert!("ctrl".parse::<MouseStroke>().is_err());
        assert!("".parse::<MouseStroke>().is_err());
    }

    #[test]
    fn displays_per_platform() {
        let s = stroke("control shift CLOSE_BRACKET");
        assert_eq!(s.display(ShortcutStyle::Linux), "Ctrl+Shift+]");
        assert_eq!(stroke("meta shift G").display(ShortcutStyle::MacOs), "⇧⌘G");
        assert_eq!(stroke("alt F7").display(ShortcutStyle::Linux), "Alt+F7");
        let chord = Shortcut::Keyboard {
            first: stroke("control K"),
            second: Some(stroke("control C")),
        };
        assert_eq!(
            chord.display(ShortcutStyle::Linux).unwrap(),
            "Ctrl+K, Ctrl+C"
        );
        assert_eq!(
            Shortcut::DoubleModifier(Modifier::Shift)
                .display(ShortcutStyle::Linux)
                .unwrap(),
            "Shift Shift"
        );
        assert_eq!(
            Shortcut::Unsupported("button4".into()).display(ShortcutStyle::Linux),
            None
        );
    }

    proptest! {
        #[test]
        fn parsing_never_panics(text in "\\PC*") {
            let _ = text.parse::<KeyStroke>();
            let _ = parse_double_modifier(&text);
            let _ = text.parse::<MouseStroke>();
        }
    }
}
