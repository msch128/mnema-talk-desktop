//! The gaming settings kept on this device: the four chords, the games list
//! and the sidepeek.
use super::games;
use super::input;
use super::shortcuts;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Chord {
    pub(super) key: u16,
    pub(super) alt: bool,
    pub(super) control: bool,
    pub(super) shift: bool,
}
impl Chord {
    pub(super) fn binding(&self) -> Result<shortcuts::Binding, String> {
        shortcuts::Binding::new(
            self.key,
            shortcuts::Modifiers {
                alt: self.alt,
                control: self.control,
                shift: self.shift,
                windows: false,
            },
        )
        .map_err(|_| "Unsupported or reserved shortcut.".into())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub(super) overlay: Chord,
    pub(super) mute: Chord,
    pub(super) deafen: Chord,
    pub(super) ptt: Chord,
    pub(super) games: Vec<String>,
    #[serde(default = "enabled_default")]
    pub(super) enabled: bool,
    #[serde(default = "opacity_default")]
    pub(super) sidepeek_opacity: u8,
}
fn enabled_default() -> bool {
    true
}
fn opacity_default() -> u8 {
    55
}
impl Default for Settings {
    fn default() -> Self {
        let chord = |key, alt| Chord {
            key,
            alt,
            control: false,
            shift: false,
        };
        Self {
            enabled: true,
            sidepeek_opacity: opacity_default(),
            overlay: chord(0x4d, true),
            mute: chord(0x4e, true),
            deafen: chord(0x44, true),
            ptt: chord(0x20, false),
            games: vec![
                "Wow.exe".into(),
                "WowClassic.exe".into(),
                "GenshinImpact.exe".into(),
                "Wardogs.exe".into(),
                "Wardogs-Win64-Shipping.exe".into(),
            ],
        }
    }
}
impl Settings {
    /// The four chords in the order the gaming state machine uses them:
    /// overlay, mute, deafen, push-to-talk.
    pub(super) fn chords(&self) -> [input::Chord; 4] {
        [&self.overlay, &self.mute, &self.deafen, &self.ptt].map(|c| input::Chord {
            key: c.key,
            alt: c.alt,
            control: c.control,
            shift: c.shift,
        })
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.sidepeek_opacity > 100 {
            return Err("Opacity must be between 0 and 100.".into());
        }
        if !self.games.is_empty() && !games::valid_game_names(&self.games) {
            return Err("Enter game executable basenames only.".into());
        }
        let keys = [&self.overlay, &self.mute, &self.deafen, &self.ptt];
        for (i, key) in keys.iter().enumerate() {
            key.binding()?;
            if keys[..i].contains(key) {
                return Err("Shortcuts must be different.".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_keep_bindings_and_new_defaults() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        value.as_object_mut().unwrap().remove("sidepeek_opacity");
        value["overlay"]["key"] = 75.into();
        let restored: Settings = serde_json::from_value(value).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.sidepeek_opacity, 55);
        assert_eq!(restored.overlay.key, 75);
        assert!(restored.validate().is_ok());
        let mut invalid = restored;
        invalid.sidepeek_opacity = 101;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn settings_hand_the_tracker_the_chords_in_state_machine_order() {
        let chords = Settings::default().chords();
        assert_eq!((chords[0].key, chords[0].alt), (0x4d, true));
        assert_eq!((chords[1].key, chords[1].alt), (0x4e, true));
        assert_eq!((chords[2].key, chords[2].alt), (0x44, true));
        assert_eq!((chords[3].key, chords[3].alt), (0x20, false));
    }

    #[test]
    fn reject_conflicting_reserved_and_path_bindings() {
        let mut s = Settings::default();
        assert!(s.validate().is_ok());
        s.games.clear();
        assert!(s.validate().is_ok());
        s.mute = s.overlay.clone();
        assert!(s.validate().is_err());
        s = Settings::default();
        s.overlay.key = 0x73;
        assert!(s.validate().is_err());
        s = Settings::default();
        s.games = vec!["../game.exe".into()];
        assert!(s.validate().is_err());
    }
}
