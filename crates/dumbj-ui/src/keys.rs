//! Converts GPUI key events into the toolkit-independent types of `dumbj-actions`.

use dumbj_actions::{Key, KeyStroke, Modifiers};

pub(crate) fn modifiers_from_gpui(modifiers: gpui::Modifiers) -> Modifiers {
    Modifiers {
        ctrl: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
        meta: modifiers.platform,
    }
}

/// The unshifted key for a symbol typed with Shift on a US layout.
///
/// GPUI reports Ctrl+Shift+] as `ctrl-}` with the Shift flag cleared, while keymaps name
/// the physical key (`control shift CLOSE_BRACKET`). Other layouts are a known gap.
fn unshifted(symbol: char) -> Option<char> {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const PLAIN: &str = r"`1234567890-=[]\;',./";
    SHIFTED
        .chars()
        .position(|c| c == symbol)
        .and_then(|index| PLAIN.chars().nth(index))
}

fn named_key(name: &str) -> Option<Key> {
    let key = match name {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "escape" => Key::Escape,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "menu" => Key::ContextMenu,
        "add" => Key::NumpadAdd,
        "subtract" => Key::NumpadSubtract,
        "multiply" => Key::NumpadMultiply,
        "divide" => Key::NumpadDivide,
        _ => {
            let number = name.strip_prefix('f')?.parse().ok()?;
            return (1..=24).contains(&number).then_some(Key::F(number));
        }
    };
    Some(key)
}

/// Converts a GPUI keystroke. Returns `None` for keys no keymap can name.
pub(crate) fn stroke_from_gpui(keystroke: &gpui::Keystroke) -> Option<KeyStroke> {
    let mut modifiers = modifiers_from_gpui(keystroke.modifiers);
    let mut chars = keystroke.key.chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) => {
            if let Some(plain) = unshifted(c) {
                modifiers.shift = true;
                Key::Char(plain)
            } else if c.is_ascii_graphic() {
                Key::Char(c.to_ascii_lowercase())
            } else {
                return None;
            }
        }
        _ => named_key(&keystroke.key)?,
    };
    Some(KeyStroke::new(modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpui_stroke(control: bool, shift: bool, alt: bool, key: &str) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control,
                shift,
                alt,
                ..Default::default()
            },
            key: key.to_owned(),
            key_char: None,
        }
    }

    fn expect(text: &str) -> Option<KeyStroke> {
        text.parse().ok()
    }

    #[test]
    fn letters_digits_and_named_keys() {
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, false, "w")),
            expect("control W")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, true, false, "f")),
            expect("control shift F")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, true, "1")),
            expect("alt 1")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, true, "f7")),
            expect("alt F7")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, true, false, "f12")),
            expect("control shift F12")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, true, false, "enter")),
            expect("shift ENTER")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, true, "up")),
            expect("alt UP")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, false, "pagedown")),
            expect("PAGE_DOWN")
        );
    }

    #[test]
    fn shifted_symbols_map_back_to_their_key() {
        // GPUI clears the Shift flag for symbols and reports the shifted character.
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, false, "}")),
            expect("control shift CLOSE_BRACKET")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, false, "{")),
            expect("control shift OPEN_BRACKET")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, false, "]")),
            expect("control CLOSE_BRACKET")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, false, "?")),
            expect("control shift SLASH")
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(true, false, true, "*")),
            expect("control alt shift 8")
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, false, "é")),
            None
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, false, "f99")),
            None
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, false, "back")),
            None
        );
        assert_eq!(
            stroke_from_gpui(&gpui_stroke(false, false, false, "")),
            None
        );
    }
}
