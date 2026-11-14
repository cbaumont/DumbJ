//! Turns raw key events into actions: single strokes, two-stroke chords and double-tapped
//! modifiers (double Shift). Toolkit independent, so it can be tested headless.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::bindings::Bindings;
use crate::keystroke::{KeyStroke, Modifier, Modifiers, Shortcut};
use crate::registry::Action;

/// The longest gap allowed between each press and release of a double-tapped modifier.
const DOUBLE_TAP_STEP: Duration = Duration::from_millis(400);

/// What a key press resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// Run this action. The key press is consumed.
    Action(Action),
    /// The stroke starts a two-stroke chord; waiting for the second stroke. Consumed.
    Pending,
    /// The stroke did not complete the pending chord. Consumed, nothing runs.
    ChordCancelled,
    /// No binding applies; the key press should go on to text input.
    Unhandled,
}

#[derive(Debug, Clone, Copy)]
enum TapState {
    Idle,
    FirstDown(Modifier, Instant),
    FirstUp(Modifier, Instant),
    SecondDown(Modifier, Instant),
}

/// A state machine that matches key events against a set of [`Bindings`].
///
/// Several actions may share a shortcut, as in IntelliJ. The caller supplies an `enabled`
/// predicate and the first enabled action in registry order wins.
#[derive(Debug)]
pub struct KeyDispatcher {
    single: HashMap<KeyStroke, Vec<Action>>,
    chords: HashMap<KeyStroke, HashMap<KeyStroke, Vec<Action>>>,
    double_modifier: HashMap<Modifier, Vec<Action>>,
    pending_chord: Option<KeyStroke>,
    tap: TapState,
}

impl KeyDispatcher {
    #[must_use]
    pub fn new(bindings: &Bindings) -> Self {
        let mut single: HashMap<_, Vec<_>> = HashMap::new();
        let mut chords: HashMap<_, HashMap<_, Vec<_>>> = HashMap::new();
        let mut double_modifier: HashMap<_, Vec<_>> = HashMap::new();
        // `Bindings::iter` is ordered by action, which keeps candidate lists deterministic.
        for (action, shortcuts) in bindings.iter() {
            for shortcut in shortcuts {
                match shortcut {
                    Shortcut::Keyboard {
                        first,
                        second: None,
                    } => {
                        single.entry(*first).or_default().push(action);
                    }
                    Shortcut::Keyboard {
                        first,
                        second: Some(second),
                    } => {
                        chords
                            .entry(*first)
                            .or_default()
                            .entry(*second)
                            .or_default()
                            .push(action);
                    }
                    Shortcut::DoubleModifier(modifier) => {
                        double_modifier.entry(*modifier).or_default().push(action);
                    }
                    Shortcut::Unsupported(_) => {}
                }
            }
        }
        Self {
            single,
            chords,
            double_modifier,
            pending_chord: None,
            tap: TapState::Idle,
        }
    }

    /// Whether the first stroke of a chord has been pressed and the second is awaited.
    #[must_use]
    pub fn is_chord_pending(&self) -> bool {
        self.pending_chord.is_some()
    }

    /// Feeds one non-modifier key press.
    pub fn key_down(&mut self, stroke: KeyStroke, enabled: impl Fn(Action) -> bool) -> Dispatch {
        self.tap = TapState::Idle;
        let first_enabled =
            |actions: Option<&Vec<Action>>| actions?.iter().copied().find(|a| enabled(*a));

        if let Some(first) = self.pending_chord.take() {
            let seconds = self.chords.get(&first);
            return match first_enabled(seconds.and_then(|seconds| seconds.get(&stroke))) {
                Some(action) => Dispatch::Action(action),
                None => Dispatch::ChordCancelled,
            };
        }
        if let Some(action) = first_enabled(self.single.get(&stroke)) {
            return Dispatch::Action(action);
        }
        let starts_chord = self.chords.get(&stroke).is_some_and(|seconds| {
            seconds
                .values()
                .any(|actions| first_enabled(Some(actions)).is_some())
        });
        if starts_chord {
            self.pending_chord = Some(stroke);
            return Dispatch::Pending;
        }
        Dispatch::Unhandled
    }

    /// Feeds a change in the held modifiers. Returns an action when a modifier has been
    /// tapped twice (press, release, press, release) with nothing else in between.
    pub fn modifiers_changed(
        &mut self,
        modifiers: Modifiers,
        now: Instant,
        enabled: impl Fn(Action) -> bool,
    ) -> Option<Action> {
        let lone = lone_modifier(modifiers);
        let in_time = |since: Instant| now.saturating_duration_since(since) <= DOUBLE_TAP_STEP;
        let restart = lone.map_or(TapState::Idle, |modifier| {
            TapState::FirstDown(modifier, now)
        });

        let (next, fired) = match self.tap {
            TapState::FirstDown(modifier, at) if modifiers.is_empty() && in_time(at) => {
                (TapState::FirstUp(modifier, now), None)
            }
            TapState::FirstUp(modifier, at) if lone == Some(modifier) && in_time(at) => {
                (TapState::SecondDown(modifier, now), None)
            }
            TapState::SecondDown(modifier, at) if modifiers.is_empty() && in_time(at) => {
                (TapState::Idle, Some(modifier))
            }
            _ => (restart, None),
        };
        self.tap = next;
        let modifier = fired?;
        self.pending_chord = None;
        self.double_modifier
            .get(&modifier)?
            .iter()
            .copied()
            .find(|action| enabled(*action))
    }
}

/// The modifier that is held, if exactly one is.
fn lone_modifier(modifiers: Modifiers) -> Option<Modifier> {
    [
        Modifier::Ctrl,
        Modifier::Alt,
        Modifier::Shift,
        Modifier::Meta,
    ]
    .into_iter()
    .find(|modifier| modifiers == Modifiers::only(*modifier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::ActionIdMap;
    use crate::keymap::KeymapLibrary;

    fn dispatcher() -> KeyDispatcher {
        let mut library = KeymapLibrary::new();
        library
            .add_xml(
                r#"<keymap name="k">
                     <action id="A"><keyboard-shortcut first-keystroke="control W"/></action>
                     <action id="B"><keyboard-shortcut first-keystroke="control W"/></action>
                     <action id="C"><keyboard-shortcut first-keystroke="control K" second-keystroke="control C"/></action>
                     <action id="D"><keyboard-gesture-shortcut keystroke="shift SHIFT" modifier="dblClick"/></action>
                   </keymap>"#,
            )
            .unwrap();
        let map = ActionIdMap::parse(
            "[actions]\nA = \"tabs.close\"\nB = \"editor.extend-selection\"\nC = \"editor.find\"\nD = \"editor.goto-line\"\n",
        )
        .unwrap();
        KeyDispatcher::new(&Bindings::new(&library.resolve("k").unwrap(), &map))
    }

    fn stroke(text: &str) -> KeyStroke {
        text.parse().unwrap()
    }

    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };

    fn all(_: Action) -> bool {
        true
    }

    /// Feeds modifier states spaced `gap_ms` apart and returns what each produced.
    fn taps(d: &mut KeyDispatcher, gap_ms: u64, states: &[Modifiers]) -> Vec<Option<Action>> {
        let start = Instant::now();
        states
            .iter()
            .zip(0u32..)
            .map(|(m, i)| d.modifiers_changed(*m, start + Duration::from_millis(gap_ms) * i, all))
            .collect()
    }

    #[test]
    fn single_stroke_picks_first_enabled_action() {
        let mut d = dispatcher();
        assert_eq!(
            d.key_down(stroke("control W"), all),
            Dispatch::Action(Action::CloseTab)
        );
        assert_eq!(
            d.key_down(stroke("control W"), |a| a != Action::CloseTab),
            Dispatch::Action(Action::ExtendSelection)
        );
        assert_eq!(
            d.key_down(stroke("control W"), |_| false),
            Dispatch::Unhandled
        );
        assert_eq!(d.key_down(stroke("W"), all), Dispatch::Unhandled);
    }

    #[test]
    fn chords_wait_for_the_second_stroke() {
        let mut d = dispatcher();
        assert_eq!(d.key_down(stroke("control K"), all), Dispatch::Pending);
        assert!(d.is_chord_pending());
        assert_eq!(
            d.key_down(stroke("control C"), all),
            Dispatch::Action(Action::Find)
        );
        assert!(!d.is_chord_pending());

        assert_eq!(d.key_down(stroke("control K"), all), Dispatch::Pending);
        assert_eq!(d.key_down(stroke("X"), all), Dispatch::ChordCancelled);
        assert_eq!(d.key_down(stroke("control C"), all), Dispatch::Unhandled);

        assert_eq!(
            d.key_down(stroke("control K"), |_| false),
            Dispatch::Unhandled,
            "a chord with no enabled action does not swallow its first stroke"
        );
    }

    #[test]
    fn double_shift_fires_on_the_second_release() {
        let mut d = dispatcher();
        let none = Modifiers::NONE;
        assert_eq!(
            taps(&mut d, 100, &[SHIFT, none, SHIFT, none]),
            [None, None, None, Some(Action::GotoLine)]
        );
        // And again straight away: the state machine has reset.
        assert_eq!(
            taps(&mut d, 100, &[SHIFT, none, SHIFT, none])[3],
            Some(Action::GotoLine)
        );
    }

    #[test]
    fn double_tap_needs_speed_the_same_modifier_and_no_keys_between() {
        let none = Modifiers::NONE;
        let mut d = dispatcher();
        assert_eq!(
            taps(&mut d, 500, &[SHIFT, none, SHIFT, none]),
            [None; 4],
            "too slow"
        );

        let mut d = dispatcher();
        assert_eq!(
            taps(&mut d, 50, &[SHIFT, none, CTRL, none]),
            [None; 4],
            "other modifier"
        );

        let mut d = dispatcher();
        let both = Modifiers {
            ctrl: true,
            shift: true,
            ..none
        };
        assert_eq!(
            taps(&mut d, 50, &[SHIFT, both, SHIFT, none, SHIFT, none])[..4],
            [None; 4]
        );

        let mut d = dispatcher();
        let start = Instant::now();
        assert_eq!(d.modifiers_changed(SHIFT, start, all), None);
        assert_eq!(d.modifiers_changed(none, start, all), None);
        assert_eq!(d.modifiers_changed(SHIFT, start, all), None);
        // Typing a capital letter in the middle cancels the gesture.
        assert_eq!(d.key_down(stroke("shift A"), all), Dispatch::Unhandled);
        assert_eq!(d.modifiers_changed(none, start, all), None);

        let mut d = dispatcher();
        assert_eq!(
            taps(&mut d, 50, &[CTRL, none, CTRL, none]),
            [None; 4],
            "unbound modifier"
        );
    }

    #[test]
    fn a_slow_tap_can_start_a_new_gesture() {
        let mut d = dispatcher();
        let none = Modifiers::NONE;
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(d.modifiers_changed(SHIFT, at(0), all), None);
        assert_eq!(
            d.modifiers_changed(none, at(1000), all),
            None,
            "held too long"
        );
        assert_eq!(d.modifiers_changed(SHIFT, at(1100), all), None);
        assert_eq!(d.modifiers_changed(none, at(1200), all), None);
        assert_eq!(d.modifiers_changed(SHIFT, at(1300), all), None);
        assert_eq!(
            d.modifiers_changed(none, at(1400), all),
            Some(Action::GotoLine)
        );
    }
}
