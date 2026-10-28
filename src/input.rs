//! Event-driven keyboard and mouse-button input for push-to-talk and the gaming
//! shortcuts. The operating system tells us when a key goes down or up; nothing
//! here polls key state, hooks another process or injects input.
//!
//! The platform source (raw input on Windows; the other systems are paused) sits behind [`InputSource`] and only ever report *which* key changed. They
//! never see text, and [`ChordTracker`] keeps no history: it holds the set of
//! keys currently down and compares it with the four configured chords, so a
//! session's keystrokes are neither stored nor logged.
//!
//! Key identity is the Windows virtual-key code on every system (the shortcut
//! settings already store VK codes).

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
            Self::Unsupported(note) | Self::Conflict(note) | Self::Failed(note) => note,
        }
    }
}

/// A source of key events.
pub trait InputSource {
    fn start(&mut self, sink: Sink) -> Result<(), InputError>;
    fn stop(&mut self);
}

/// Where no system source exists. The Linux and macOS desktop clients are paused,
/// so only Windows has a source.
pub struct UnavailableSource;

impl InputSource for UnavailableSource {
    fn start(&mut self, _sink: Sink) -> Result<(), InputError> {
        Err(InputError::Unsupported(
            "Shortcuts and push-to-talk outside Mnema are available on Windows only.",
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

// Raw Input constants that are macros in winuser.h / wtsapi32.h, not typed API
// metadata. Kept here, next to the pure decoding below, so both Windows sources
// (the shipped one and the fixture owner) read a report the same way.
pub const RI_KEY_BREAK: u16 = 0x1;
pub const RI_KEY_E0: u16 = 0x2;
pub const RI_KEY_E1: u16 = 0x4;
/// `usButtonFlags` bits of the two extra mouse buttons (4 and 5), down and up.
pub const RI_MOUSE_EXTRA_BUTTONS: u16 = 0x40 | 0x80 | 0x100 | 0x200;

/// What one Raw Input keyboard report means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyReport {
    Key(KeyEvent),
    /// Not a key change (the "fake" key Windows sends around some sequences).
    Ignore,
    /// A report that cannot be trusted (unknown flags, a keyboard overrun, a
    /// Shift of unknown side): forget every key rather than risk a stuck one.
    Reset,
}

/// Decodes the three fields of a `RAWKEYBOARD` report.
pub fn decode_keyboard(vkey: u16, make_code: u16, flags: u16) -> KeyReport {
    if vkey == 0 || vkey == 0xff {
        return KeyReport::Ignore;
    }
    if make_code == 0xff || flags & !(RI_KEY_BREAK | RI_KEY_E0 | RI_KEY_E1) != 0 {
        return KeyReport::Reset;
    }
    if vkey == VK_SHIFT && !matches!(make_code, 0x2a | 0x36) {
        return KeyReport::Reset;
    }
    KeyReport::Key(KeyEvent {
        key: side_specific(vkey, make_code, flags & RI_KEY_E0 != 0),
        down: flags & RI_KEY_BREAK == 0,
    })
}

/// The changes of buttons 4 and 5 in a mouse report's `usButtonFlags`. Movement,
/// position and wheel are never read.
pub fn decode_mouse_buttons(button_flags: u16) -> impl Iterator<Item = KeyEvent> {
    [
        (0x40, VK_XBUTTON1, true),
        (0x80, VK_XBUTTON1, false),
        (0x100, VK_XBUTTON2, true),
        (0x200, VK_XBUTTON2, false),
    ]
    .into_iter()
    .filter(move |(flag, _, _)| button_flags & flag != 0)
    .map(|(_, key, down)| KeyEvent { key, down })
}

/// `WM_WTSSESSION_CHANGE` codes after which key-up events may have gone to
/// another desktop or session: lock and unlock, console or remote disconnect
/// and logoff.
pub fn session_change_loses_keys(code: u32) -> bool {
    matches!(code, 0x2 | 0x4 | 0x6 | 0x7 | 0x8)
}

/// `WM_POWERBROADCAST` codes (`PBT_APMSUSPEND`, `PBT_APMRESUMESUSPEND`,
/// `PBT_APMRESUMEAUTOMATIC`): nothing is delivered while the machine sleeps.
pub fn power_change_loses_keys(code: u32) -> bool {
    matches!(code, 0x4 | 0x7 | 0x12)
}

/// Test double: pushes whatever the test emits.
#[cfg(test)]
#[derive(Default)]
pub struct FakeInput {
    sink: Option<Sink>,
    pub running: bool,
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
    fn keyboard_reports_decode_to_side_specific_keys() {
        let key = |key, down| KeyReport::Key(KeyEvent { key, down });
        assert_eq!(decode_keyboard(0x41, 0x1e, 0), key(0x41, true));
        assert_eq!(decode_keyboard(0x41, 0x1e, RI_KEY_BREAK), key(0x41, false));
        assert_eq!(decode_keyboard(VK_SHIFT, 0x2a, 0), key(VK_LSHIFT, true));
        assert_eq!(
            decode_keyboard(VK_SHIFT, 0x36, RI_KEY_BREAK),
            key(VK_RSHIFT, false)
        );
        assert_eq!(
            decode_keyboard(VK_CONTROL, 0x1d, RI_KEY_E0),
            key(VK_RCONTROL, true)
        );
        assert_eq!(
            decode_keyboard(0x13, 0x45, RI_KEY_E1),
            key(0x13, true),
            "Pause"
        );
    }

    #[test]
    fn untrustworthy_keyboard_reports_reset_and_fake_keys_are_ignored() {
        assert_eq!(decode_keyboard(0xff, 0x2a, RI_KEY_E0), KeyReport::Ignore);
        assert_eq!(decode_keyboard(0, 0, 0), KeyReport::Ignore);
        assert_eq!(decode_keyboard(0x41, 0xff, 0), KeyReport::Reset, "overrun");
        assert_eq!(
            decode_keyboard(0x41, 0x1e, 0x8),
            KeyReport::Reset,
            "unknown flag"
        );
        assert_eq!(
            decode_keyboard(VK_SHIFT, 0x1e, 0),
            KeyReport::Reset,
            "unknown Shift side"
        );
    }

    #[test]
    fn extra_mouse_buttons_decode_and_other_mouse_data_is_dropped() {
        let events: Vec<_> = decode_mouse_buttons(0x40 | 0x200).collect();
        assert_eq!(
            events,
            [
                KeyEvent {
                    key: VK_XBUTTON1,
                    down: true
                },
                KeyEvent {
                    key: VK_XBUTTON2,
                    down: false
                },
            ]
        );
        // Buttons 1-3 and the wheel are not ours.
        assert_eq!(decode_mouse_buttons(0x1 | 0x2 | 0x400).count(), 0);
        assert_eq!(RI_MOUSE_EXTRA_BUTTONS, 0x3c0);
    }

    #[test]
    fn session_and_power_changes_that_lose_key_ups_are_recognised() {
        // lock 7, unlock 8, console disconnect 2, remote disconnect 4, logoff 6
        for code in [2, 4, 6, 7, 8] {
            assert!(session_change_loses_keys(code), "{code}");
        }
        // connects, logon, remote control: keys are still tracked
        for code in [1, 3, 5, 9, 0xa] {
            assert!(!session_change_loses_keys(code), "{code}");
        }
        for code in [0x4, 0x7, 0x12] {
            assert!(power_change_loses_keys(code), "{code}");
        }
        assert!(!power_change_loses_keys(0xa), "power status change");
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
}
