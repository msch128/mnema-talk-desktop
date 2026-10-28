//! The one-time hint for games in exclusive fullscreen.
//!
//! The sidepeek and the overlay are ordinary topmost windows. A game that owns
//! the display in exclusive fullscreen is presented straight to the screen, so no
//! other window can be drawn over it. Telling the person once, and suggesting
//! borderless windowed mode (which the sidepeek and overlay can be drawn over),
//! is all this client does about it: it never touches the game's graphics.
//!
//! The state comes from the shell's own cheap answer to "is something running
//! full screen?" (`SHQueryUserNotificationState` on Windows), not from looking
//! at the game or its window.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    /// Nothing that blocks windows: a normal desktop, a windowed or borderless game.
    Normal,
    /// A Direct3D application owns the display in exclusive fullscreen.
    ExclusiveFullscreen,
}

/// `QUERY_USER_NOTIFICATION_STATE`: 3 is `QUNS_RUNNING_D3D_FULL_SCREEN`. The
/// others (a full-screen window such as borderless windowed, presentation mode,
/// quiet time, ...) can all be drawn over, so they are `Normal`.
pub fn from_quns(code: i32) -> ScreenState {
    if code == 3 {
        ScreenState::ExclusiveFullscreen
    } else {
        ScreenState::Normal
    }
}

pub const HINT_TITLE: &str = "Mnema cannot draw over this game";
pub const HINT_BODY: &str = "The game uses exclusive fullscreen, so the sidepeek and overlay cannot be shown over it. Switch the game to borderless windowed (also called windowed fullscreen) to see them. Push-to-talk and shortcuts keep working.";

/// How many looks in a row must see exclusive fullscreen: a game that is only
/// switching modes for a moment does not count.
const LOOKS: u32 = 2;

/// Decides when the hint is due. Pure state; the caller does the looking and the
/// showing.
#[derive(Debug, Default)]
pub struct HintGate {
    shown: bool,
    streak: u32,
}

impl HintGate {
    pub fn new(already_shown: bool) -> Self {
        Self {
            shown: already_shown,
            streak: 0,
        }
    }

    /// Whether the hint has been given (this run or an earlier one).
    pub fn done(&self) -> bool {
        self.shown
    }

    /// One look. True exactly once: when a game has been in front in exclusive
    /// fullscreen for [`LOOKS`] looks in a row and the hint was never given.
    pub fn look(&mut self, game_in_front: bool, state: ScreenState) -> bool {
        if self.shown {
            return false;
        }
        if game_in_front && state == ScreenState::ExclusiveFullscreen {
            self.streak += 1;
        } else {
            self.streak = 0;
        }
        if self.streak >= LOOKS {
            self.shown = true;
            return true;
        }
        false
    }
}

const FILE: &str = "gaming-hints.json";

/// Whether the hint was given in an earlier run.
pub fn was_shown(config_dir: &Path) -> bool {
    std::fs::read(config_dir.join(FILE))
        .ok()
        .filter(|bytes| bytes.len() <= 1024)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("exclusive_fullscreen")?.as_bool())
        .unwrap_or(false)
}

/// Remembers that the hint was given, so it is never repeated. The marker is
/// written to a temporary file and renamed into place, so a crash never leaves
/// half a file. An error means the hint will be given again on the next start.
pub fn remember_shown(config_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let temporary = config_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&temporary, br#"{"exclusive_fullscreen":true}"#)?;
    std::fs::rename(&temporary, config_dir.join(FILE)).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ScreenState::{ExclusiveFullscreen as Exclusive, Normal};

    #[test]
    fn only_the_d3d_fullscreen_state_blocks_windows() {
        assert_eq!(from_quns(3), Exclusive);
        // 1 not present, 2 busy (any full-screen window), 4 presentation, 5 accepts
        // notifications, 6 quiet time, 7 app: all can be drawn over.
        for code in [-1, 0, 1, 2, 4, 5, 6, 7, 8] {
            assert_eq!(from_quns(code), Normal, "{code}");
        }
    }

    #[test]
    fn the_hint_fires_once_after_two_looks_in_a_row() {
        let mut gate = HintGate::default();
        assert!(!gate.look(true, Exclusive));
        assert!(gate.look(true, Exclusive));
        assert!(gate.done());
        for _ in 0..5 {
            assert!(!gate.look(true, Exclusive), "never twice");
        }
    }

    #[test]
    fn a_blip_a_missing_game_or_a_borderless_game_never_fires_it() {
        let mut gate = HintGate::default();
        assert!(!gate.look(true, Exclusive));
        assert!(!gate.look(true, Normal), "the streak is broken");
        assert!(!gate.look(true, Exclusive));
        assert!(
            !gate.look(false, Exclusive),
            "exclusive fullscreen of something that is no game"
        );
        assert!(!gate.look(true, Exclusive));
        assert!(!gate.done());
        assert!(gate.look(true, Exclusive));
    }

    #[test]
    fn a_hint_given_in_an_earlier_run_is_never_repeated() {
        let mut gate = HintGate::new(true);
        for _ in 0..4 {
            assert!(!gate.look(true, Exclusive));
        }
    }

    #[test]
    fn the_hint_text_names_the_remedy_and_what_keeps_working() {
        assert!(HINT_BODY.contains("borderless windowed"));
        assert!(HINT_BODY.contains("Push-to-talk"));
        assert!(HINT_BODY.len() < 240, "fits a toast: {}", HINT_BODY.len());
    }

    #[test]
    fn a_marker_that_cannot_be_written_is_reported_not_swallowed() {
        let file = std::env::temp_dir().join(format!("mnema-hints-file-{}", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        // The "directory" is a regular file, so the marker cannot be stored.
        assert!(remember_shown(&file).is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_given_hint_is_remembered_across_runs_and_junk_means_not_given() {
        let dir = std::env::temp_dir().join(format!("mnema-hints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!was_shown(&dir), "no file yet");
        remember_shown(&dir).unwrap();
        assert!(was_shown(&dir));
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert!(!was_shown(&dir));
        std::fs::write(dir.join(FILE), r#"{"exclusive_fullscreen":"yes"}"#).unwrap();
        assert!(!was_shown(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
