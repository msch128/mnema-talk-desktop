//! Event-driven keyboard and mouse-button input for push-to-talk and the gaming
//! shortcuts. The operating system tells us when a key goes down or up; nothing
//! here polls key state, hooks another process or injects input.
//!
//! The platform sources (raw input on Windows, a listen-only event tap on macOS)
//! sit behind [`InputSource`] and only ever report *which* key changed. They
//! never see text, and [`ChordTracker`] keeps no history: it holds the set of
//! keys currently down and compares it with the four configured chords, so a
//! session's keystrokes are neither stored nor logged.
//!
//! Key identity is the Windows virtual-key code on every system (the shortcut
//! settings already store VK codes); macOS key codes are translated here.

/// Windows virtual-key codes used by chords and modifiers.
pub const VK_XBUTTON1: u16 = 0x05;
pub const VK_XBUTTON2: u16 = 0x06;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_LWIN: u16 = 0x5b;
pub const VK_RWIN: u16 = 0x5c;
pub const VK_LSHIFT: u16 = 0xa0;
pub const VK_RSHIFT: u16 = 0xa1;
pub const VK_LCONTROL: u16 = 0xa2;
pub const VK_RCONTROL: u16 = 0xa3;
pub const VK_LMENU: u16 = 0xa4;
pub const VK_RMENU: u16 = 0xa5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: u16,
    pub down: bool,
}

/// What a source reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    /// Key-up events may have been lost (screen lock, a disabled event tap, a
    /// device change): forget every key.
    Reset,
}

pub type Sink = Box<dyn Fn(InputEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    /// The system asks the user to allow it (macOS Input Monitoring).
    NeedsPermission,
    /// This system has no way to see keys outside the app. Say why.
    Unsupported(&'static str),
    /// Another part of the app already owns the system registration.
    Conflict(&'static str),
    Failed(&'static str),
}

impl InputError {
    /// A short, plain reason the user can be shown.
    pub fn note(&self) -> &'static str {
        match self {
            Self::NeedsPermission => NEEDS_PERMISSION_NOTE,
            Self::Unsupported(note) | Self::Conflict(note) | Self::Failed(note) => note,
        }
    }
}

pub const NEEDS_PERMISSION_NOTE: &str = "Push-to-talk and shortcuts outside Mnema need Input Monitoring. Allow Mnema in System Settings > Privacy & Security > Input Monitoring. Everything else works without it.";

/// A source of key events. `start` must not show any system prompt: asking the
/// user is [`InputSource::request_access`], called once when the user turns
/// push-to-talk on.
pub trait InputSource {
    fn start(&mut self, sink: Sink) -> Result<(), InputError>;
    fn stop(&mut self);
    fn request_access(&mut self) {}
}

/// Where no system source exists. Linux has no gaming host: the Electron client
/// has no overlay or global shortcuts, and on Wayland the only portable route
/// (the GlobalShortcuts portal) delivers presses without a guaranteed release.
pub struct UnavailableSource;

impl InputSource for UnavailableSource {
    fn start(&mut self, _sink: Sink) -> Result<(), InputError> {
        Err(InputError::Unsupported(
            "Shortcuts and push-to-talk work only while Mnema is in front on this system.",
        ))
    }
    fn stop(&mut self) {}
}

/// The chords the gaming shortcuts and push-to-talk are configured with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Chord {
    pub key: u16,
    pub alt: bool,
    pub control: bool,
    pub shift: bool,
}

pub const CHORDS: usize = 4;

/// Which keys are down right now, matched against the configured chords.
///
/// A chord is "down" when its key is down, exactly its modifiers (alt, control,
/// shift) are down and no Windows/Command key is down. A press that begins and
/// ends between two looks at the tracker is latched once, so a short tap is
/// never lost.
#[derive(Debug, Clone)]
pub struct ChordTracker {
    down: [u64; 4],
    chords: [Chord; CHORDS],
    matched: [bool; CHORDS],
    latched: [bool; CHORDS],
}

impl Default for ChordTracker {
    fn default() -> Self {
        Self {
            down: [0; 4],
            chords: [Chord::default(); CHORDS],
            matched: [false; CHORDS],
            latched: [false; CHORDS],
        }
    }
}

impl ChordTracker {
    fn is_down(&self, key: u16) -> bool {
        key < 256 && self.down[usize::from(key) / 64] & (1 << (key % 64)) != 0
    }

    fn set(&mut self, key: u16, down: bool) {
        if key >= 256 {
            return;
        }
        let bit = 1u64 << (key % 64);
        let word = &mut self.down[usize::from(key) / 64];
        if down {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    fn any(&self, keys: &[u16]) -> bool {
        keys.iter().any(|k| self.is_down(*k))
    }

    fn evaluate(&self, chord: &Chord) -> bool {
        chord.key != 0
            && !self.any(&[VK_LWIN, VK_RWIN])
            && chord.alt == self.any(&[VK_MENU, VK_LMENU, VK_RMENU])
            && chord.control == self.any(&[VK_CONTROL, VK_LCONTROL, VK_RCONTROL])
            && chord.shift == self.any(&[VK_SHIFT, VK_LSHIFT, VK_RSHIFT])
            && self.is_down(chord.key)
    }

    fn refresh(&mut self) -> bool {
        let mut changed = false;
        for i in 0..CHORDS {
            let now = self.evaluate(&self.chords[i]);
            if now && !self.matched[i] {
                self.latched[i] = true;
            }
            changed |= now != self.matched[i];
            self.matched[i] = now;
        }
        changed
    }

    /// Replaces the chords (the settings changed). Held keys stay held.
    pub fn set_chords(&mut self, chords: [Chord; CHORDS]) {
        self.chords = chords;
        self.latched = [false; CHORDS];
        self.matched = [false; CHORDS];
        // A chord that is already held when it is configured is not a press.
        for i in 0..CHORDS {
            self.matched[i] = self.evaluate(&self.chords[i]);
        }
    }

    /// Forgets every key. True when a chord had been down.
    pub fn clear(&mut self) -> bool {
        self.down = [0; 4];
        self.latched = [false; CHORDS];
        self.refresh()
    }

    /// Applies one event. True when a chord went down or up, which is the only
    /// time a consumer needs to look again.
    pub fn apply(&mut self, event: InputEvent) -> bool {
        match event {
            InputEvent::Reset => self.clear(),
            InputEvent::Key(KeyEvent { key, down }) => {
                self.set(key, down);
                self.refresh()
            }
        }
    }

    /// The chords down now, plus any that went down and up since the last
    /// sample. The second value says a latched press still has to be followed by
    /// a sample that sees it released.
    pub fn sample(&mut self) -> ([bool; CHORDS], bool) {
        let mut keys = self.matched;
        let mut again = false;
        for ((key, latched), matched) in keys.iter_mut().zip(&mut self.latched).zip(self.matched) {
            if *latched {
                again |= !matched;
                *key = true;
                *latched = false;
            }
        }
        (keys, again)
    }
}

/// Windows raw input reports Shift, Control and Alt with generic codes; the
/// scan code and the extended-key flag say which side. Mapping to the
/// left/right codes keeps one released side from releasing the other.
pub fn side_specific(vkey: u16, make_code: u16, extended: bool) -> u16 {
    match vkey {
        VK_SHIFT => {
            if make_code == 0x36 {
                VK_RSHIFT
            } else {
                VK_LSHIFT
            }
        }
        VK_CONTROL => {
            if extended {
                VK_RCONTROL
            } else {
                VK_LCONTROL
            }
        }
        VK_MENU => {
            if extended {
                VK_RMENU
            } else {
                VK_LMENU
            }
        }
        other => other,
    }
}

/// macOS virtual key code (`kVK_*`) to the Windows virtual-key code the chords
/// use. Only the keys a chord can contain, plus the modifiers.
pub fn vk_from_mac_key(code: u16) -> Option<u16> {
    const LETTERS: [(u16, u8); 26] = [
        (0x00, b'A'),
        (0x0b, b'B'),
        (0x08, b'C'),
        (0x02, b'D'),
        (0x0e, b'E'),
        (0x03, b'F'),
        (0x05, b'G'),
        (0x04, b'H'),
        (0x22, b'I'),
        (0x26, b'J'),
        (0x28, b'K'),
        (0x25, b'L'),
        (0x2e, b'M'),
        (0x2d, b'N'),
        (0x1f, b'O'),
        (0x23, b'P'),
        (0x0c, b'Q'),
        (0x0f, b'R'),
        (0x01, b'S'),
        (0x11, b'T'),
        (0x20, b'U'),
        (0x09, b'V'),
        (0x0d, b'W'),
        (0x07, b'X'),
        (0x10, b'Y'),
        (0x06, b'Z'),
    ];
    // ANSI digit row: 1 2 3 4 5 6 7 8 9 0.
    const DIGITS: [(u16, u8); 10] = [
        (0x12, b'1'),
        (0x13, b'2'),
        (0x14, b'3'),
        (0x15, b'4'),
        (0x17, b'5'),
        (0x16, b'6'),
        (0x1a, b'7'),
        (0x1c, b'8'),
        (0x19, b'9'),
        (0x1d, b'0'),
    ];
    // F1..F20 in order.
    const FUNCTION: [u16; 20] = [
        0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f, 0x69, 0x6b, 0x71,
        0x6a, 0x40, 0x4f, 0x50, 0x5a,
    ];
    if code == 0x31 {
        return Some(0x20);
    }
    if let Some((_, ascii)) = LETTERS
        .iter()
        .chain(DIGITS.iter())
        .find(|(c, _)| *c == code)
    {
        return Some(u16::from(*ascii));
    }
    if let Some(index) = FUNCTION.iter().position(|c| *c == code) {
        return Some(0x70 + index as u16);
    }
    mac_modifier(code).map(|(vk, _)| vk)
}

/// A modifier key code with the device-dependent flag bit that says whether
/// that very key (not just its kind) is held.
fn mac_modifier(code: u16) -> Option<(u16, u64)> {
    Some(match code {
        0x38 => (VK_LSHIFT, 0x2),
        0x3c => (VK_RSHIFT, 0x4),
        0x3b => (VK_LCONTROL, 0x1),
        0x3e => (VK_RCONTROL, 0x2000),
        0x3a => (VK_LMENU, 0x20),
        0x3d => (VK_RMENU, 0x40),
        0x37 => (VK_LWIN, 0x8),
        0x36 => (VK_RWIN, 0x10),
        _ => return None,
    })
}

/// A macOS `flagsChanged` event: which modifier key moved and whether it is
/// down, from the event's device-dependent flag bits.
pub fn mac_modifier_event(code: u16, flags: u64) -> Option<KeyEvent> {
    let (key, bit) = mac_modifier(code)?;
    Some(KeyEvent {
        key,
        down: flags & bit != 0,
    })
}

/// macOS "other" mouse button number (3 = back, 4 = forward) to the chord code.
pub fn vk_from_mac_button(button: i64) -> Option<u16> {
    match button {
        3 => Some(VK_XBUTTON1),
        4 => Some(VK_XBUTTON2),
        _ => None,
    }
}

/// Test double: pushes whatever the test emits.
#[cfg(test)]
#[derive(Default)]
pub struct FakeInput {
    sink: Option<Sink>,
    pub running: bool,
    pub access_requests: u32,
    pub deny: Option<InputError>,
}

#[cfg(test)]
impl FakeInput {
    pub fn emit(&self, event: InputEvent) {
        if let Some(sink) = &self.sink {
            sink(event);
        }
    }
}

#[cfg(test)]
impl InputSource for FakeInput {
    fn start(&mut self, sink: Sink) -> Result<(), InputError> {
        if let Some(error) = self.deny.clone() {
            return Err(error);
        }
        self.running = true;
        self.sink = Some(sink);
        Ok(())
    }
    fn stop(&mut self) {
        self.running = false;
        self.sink = None;
    }
    fn request_access(&mut self) {
        self.access_requests += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const SPACE: u16 = 0x20;
    const M: u16 = 0x4d;

    fn key(key: u16, down: bool) -> InputEvent {
        InputEvent::Key(KeyEvent { key, down })
    }

    fn tracker() -> ChordTracker {
        let mut t = ChordTracker::default();
        t.set_chords([
            Chord {
                key: M,
                alt: true,
                ..Chord::default()
            },
            Chord {
                key: 0x4e,
                alt: true,
                ..Chord::default()
            },
            Chord {
                key: 0x44,
                alt: true,
                ..Chord::default()
            },
            Chord {
                key: SPACE,
                ..Chord::default()
            },
        ]);
        t
    }

    #[test]
    fn a_chord_is_down_only_with_exactly_its_modifiers() {
        let mut t = tracker();
        assert!(!t.apply(key(M, true)), "M alone is not Alt+M");
        assert_eq!(t.sample().0, [false; 4]);
        t.apply(key(M, false));
        assert!(!t.apply(key(VK_LMENU, true)));
        assert!(t.apply(key(M, true)));
        assert_eq!(t.sample().0, [true, false, false, false]);
        // Adding Shift breaks the exact match; dropping it restores it.
        assert!(t.apply(key(VK_LSHIFT, true)));
        assert_eq!(t.sample().0, [false; 4]);
        assert!(t.apply(key(VK_LSHIFT, false)));
        assert_eq!(t.sample().0, [true, false, false, false]);
        assert!(t.apply(key(M, false)));
        assert_eq!(t.sample().0, [false; 4]);
    }

    #[test]
    fn plain_push_to_talk_key_ignores_nothing_but_the_windows_key() {
        let mut t = tracker();
        assert!(t.apply(key(SPACE, true)));
        assert!(t.sample().0[3]);
        assert!(t.apply(key(VK_LWIN, true)), "Win+Space is not push-to-talk");
        assert!(!t.sample().0[3]);
        assert!(t.apply(key(VK_LWIN, false)));
        assert!(t.sample().0[3]);
        // Any modifier makes it a different chord.
        assert!(t.apply(key(VK_LCONTROL, true)));
        assert!(!t.sample().0[3]);
    }

    #[test]
    fn key_repeat_and_unrelated_typing_do_not_wake_anyone() {
        let mut t = tracker();
        assert!(t.apply(key(SPACE, true)));
        for _ in 0..5 {
            assert!(!t.apply(key(SPACE, true)), "auto-repeat is not a change");
        }
        for typed in *b"HI!" {
            assert!(!t.apply(key(u16::from(typed), true)));
            assert!(!t.apply(key(u16::from(typed), false)));
        }
        assert!(t.apply(key(SPACE, false)));
    }

    #[test]
    fn a_tap_between_two_samples_is_latched_once_and_then_released() {
        let mut t = tracker();
        assert!(t.apply(key(SPACE, true)));
        assert!(t.apply(key(SPACE, false)));
        let (keys, again) = t.sample();
        assert!(keys[3], "the tap is seen");
        assert!(again, "and the release still has to be sampled");
        let (keys, again) = t.sample();
        assert!(!keys[3]);
        assert!(!again);
    }

    #[test]
    fn a_held_chord_is_not_latched_twice() {
        let mut t = tracker();
        t.apply(key(SPACE, true));
        assert_eq!(t.sample(), ([false, false, false, true], false));
        assert_eq!(t.sample(), ([false, false, false, true], false));
        t.apply(key(SPACE, false));
        assert_eq!(t.sample().0, [false; 4]);
    }

    #[test]
    fn left_and_right_modifiers_release_independently() {
        let mut t = tracker();
        t.apply(key(VK_LMENU, true));
        t.apply(key(VK_RMENU, true));
        t.apply(key(M, true));
        assert!(t.sample().0[0]);
        t.apply(key(VK_LMENU, false));
        assert!(t.sample().0[0], "the right Alt is still down");
        t.apply(key(VK_RMENU, false));
        assert!(!t.sample().0[0]);
    }

    #[test]
    fn reset_and_clear_drop_every_key_and_report_a_released_chord() {
        let mut t = tracker();
        t.apply(key(SPACE, true));
        let _ = t.sample();
        assert!(
            t.apply(InputEvent::Reset),
            "the chord that was down went up"
        );
        assert_eq!(t.sample(), ([false; 4], false));
        assert!(!t.clear());
        // A stuck key-up that never came does not survive a reset.
        t.apply(key(M, true));
        t.apply(InputEvent::Reset);
        t.apply(key(VK_LMENU, true));
        assert!(!t.sample().0[0]);
    }

    #[test]
    fn reconfiguring_does_not_treat_a_held_chord_as_a_new_press() {
        let mut t = ChordTracker::default();
        t.apply(key(SPACE, true));
        t.set_chords([
            Chord::default(),
            Chord::default(),
            Chord::default(),
            Chord {
                key: SPACE,
                ..Chord::default()
            },
        ]);
        assert_eq!(t.sample().0, [false, false, false, true]);
        // An unset chord (key 0) never matches.
        assert!(!t.apply(key(0, true)));
    }

    #[test]
    fn mouse_side_buttons_work_as_chord_keys() {
        let mut t = ChordTracker::default();
        t.set_chords([
            Chord {
                key: VK_XBUTTON1,
                ..Chord::default()
            },
            Chord::default(),
            Chord::default(),
            Chord {
                key: VK_XBUTTON2,
                shift: true,
                ..Chord::default()
            },
        ]);
        assert!(t.apply(key(VK_XBUTTON1, true)));
        assert!(t.sample().0[0]);
        // Shift turns the first chord off; the second needs Shift.
        assert!(t.apply(key(VK_LSHIFT, true)));
        assert!(!t.sample().0[0]);
        assert!(t.apply(key(VK_XBUTTON2, true)));
        assert!(t.sample().0[3]);
    }

    #[test]
    fn out_of_range_keys_are_ignored() {
        let mut t = tracker();
        assert!(!t.apply(key(300, true)));
        assert!(!t.apply(key(u16::MAX, true)));
    }

    #[test]
    fn raw_input_modifiers_get_their_side() {
        assert_eq!(side_specific(VK_SHIFT, 0x2a, false), VK_LSHIFT);
        assert_eq!(side_specific(VK_SHIFT, 0x36, false), VK_RSHIFT);
        assert_eq!(side_specific(VK_CONTROL, 0x1d, false), VK_LCONTROL);
        assert_eq!(side_specific(VK_CONTROL, 0x1d, true), VK_RCONTROL);
        assert_eq!(side_specific(VK_MENU, 0x38, false), VK_LMENU);
        assert_eq!(side_specific(VK_MENU, 0x38, true), VK_RMENU);
        assert_eq!(side_specific(0x41, 0x1e, false), 0x41);
    }

    #[test]
    fn mac_key_codes_map_to_the_chord_keys() {
        // Every letter, digit and function key a chord may use is reachable once.
        let mut seen = std::collections::HashSet::new();
        for code in 0u16..0x80 {
            if let Some(vk) = vk_from_mac_key(code) {
                assert!(seen.insert(vk), "two mac keys map to {vk:#x}");
            }
        }
        for vk in (b'A'..=b'Z').chain(b'0'..=b'9') {
            assert!(seen.contains(&u16::from(vk)), "{}", vk as char);
        }
        for vk in 0x70..=0x83 {
            assert!(seen.contains(&vk), "F{}", vk - 0x6f);
        }
        assert_eq!(vk_from_mac_key(0x31), Some(0x20), "space");
        assert_eq!(vk_from_mac_key(0x2e), Some(u16::from(b'M')));
        assert_eq!(vk_from_mac_key(0x7a), Some(0x70), "F1");
        assert_eq!(vk_from_mac_key(0x24), None, "return is not a chord key");
        assert_eq!(vk_from_mac_key(0x35), None, "escape is not a chord key");
    }

    #[test]
    fn mac_modifier_events_follow_the_device_flags_of_the_very_key() {
        // Left and right Shift: releasing one while the other is held.
        let both = 0x2 | 0x4;
        assert_eq!(
            mac_modifier_event(0x38, both),
            Some(KeyEvent {
                key: VK_LSHIFT,
                down: true
            })
        );
        assert_eq!(
            mac_modifier_event(0x38, 0x4),
            Some(KeyEvent {
                key: VK_LSHIFT,
                down: false
            })
        );
        assert_eq!(
            mac_modifier_event(0x3a, 0x20),
            Some(KeyEvent {
                key: VK_LMENU,
                down: true
            })
        );
        assert_eq!(
            mac_modifier_event(0x37, 0x8),
            Some(KeyEvent {
                key: VK_LWIN,
                down: true
            })
        );
        assert_eq!(mac_modifier_event(0x00, 0xffff), None);
    }

    #[test]
    fn mac_buttons_four_and_five_are_the_side_buttons() {
        assert_eq!(vk_from_mac_button(3), Some(VK_XBUTTON1));
        assert_eq!(vk_from_mac_button(4), Some(VK_XBUTTON2));
        assert_eq!(vk_from_mac_button(2), None);
    }

    #[test]
    fn fake_source_feeds_a_tracker_through_the_sink_and_stops_cleanly() {
        let shared = Arc::new(Mutex::new(tracker()));
        let woke = Arc::new(Mutex::new(0));
        let mut source = FakeInput::default();
        let (t, w) = (shared.clone(), woke.clone());
        source
            .start(Box::new(move |event| {
                if t.lock().unwrap().apply(event) {
                    *w.lock().unwrap() += 1;
                }
            }))
            .unwrap();
        source.emit(key(SPACE, true));
        source.emit(key(b'X' as u16, true));
        source.emit(key(SPACE, false));
        assert_eq!(*woke.lock().unwrap(), 2);
        source.stop();
        source.emit(key(SPACE, true));
        assert_eq!(*woke.lock().unwrap(), 2);
        assert!(!source.running);
    }

    #[test]
    fn permission_errors_explain_themselves_and_start_never_prompts() {
        let mut source = FakeInput {
            deny: Some(InputError::NeedsPermission),
            ..FakeInput::default()
        };
        let error = source.start(Box::new(|_| {})).unwrap_err();
        assert!(error.note().contains("Input Monitoring"));
        assert_eq!(source.access_requests, 0);
        source.request_access();
        assert_eq!(source.access_requests, 1);
        assert!(InputError::Unsupported("x").note() == "x");
    }
}
