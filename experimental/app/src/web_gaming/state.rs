//! The shared state of gaming: which server owns the Talk, what the person's
//! settings are, and the state machine that turns key edges into commands.
use super::command::Command;
#[cfg(windows)]
use super::native::GameLease;
use super::settings::Settings;
use super::surface::close_surfaces;
use super::voice::Voice;
use super::{foreground, fullscreen, game_library_scan, input};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::Manager;

pub struct Gaming {
    /// The app origin the sessions are bound to (`local_origin` of their pages).
    pub(super) origin: Option<String>,
    /// The bridge token of each server's page, by server id.
    pub(super) tokens: HashMap<String, String>,
    /// The server that owns the Talk gaming is attached to.
    pub(super) owner: Option<String>,
    pub(super) voice: Voice,
    pub(super) updated: Instant,
    pub(super) settings: Settings,
    pub(super) overlay: bool,
    pub(super) active: bool,
    pub(super) previous: [bool; 4],
    pub(super) ptt_down: bool,
    pub(super) ptt_armed: bool,
    pub(super) last_tick: Instant,
    /// The application in front, as the operating system last announced it.
    /// Updated by the foreground source's events, never by polling.
    pub(super) foreground: Option<foreground::Foreground>,
    /// File name of the last other application that was in front, offered only
    /// when the person asks to add it as a game. Never a path.
    pub(super) candidate: Option<String>,
    /// Why game detection is off on this system, when it is.
    pub(super) detection_note: Option<&'static str>,
    /// Keys that are down, matched against the chords; fed by the input source.
    pub(super) keys: std::sync::Arc<Mutex<input::ChordTracker>>,
    /// Why shortcuts and push-to-talk do not work outside Mnema, when they do not.
    pub(super) input_note: Option<&'static str>,
    /// When to tell the person, once, that a game in exclusive fullscreen hides our windows.
    pub(super) hint: fullscreen::HintGate,
    /// The hint was given during this run.
    pub(super) exclusive_hint: bool,
    /// Install folders of the launcher libraries, scanned off the hot path.
    pub(super) library: std::sync::Arc<game_library_scan::Library>,
    #[cfg(windows)]
    pub(super) game: Option<GameLease>,
}
impl Default for Gaming {
    fn default() -> Self {
        Self {
            origin: None,
            tokens: HashMap::new(),
            owner: None,
            voice: Voice::default(),
            updated: Instant::now(),
            settings: Settings::default(),
            overlay: false,
            active: false,
            previous: [true; 4],
            ptt_down: false,
            ptt_armed: false,
            last_tick: Instant::now(),
            foreground: None,
            candidate: None,
            detection_note: None,
            keys: Default::default(),
            input_note: None,
            hint: Default::default(),
            exclusive_hint: false,
            library: Default::default(),
            #[cfg(windows)]
            game: None,
        }
    }
}

impl Gaming {
    /// A page reports its Talk. The page must hold the token of its own
    /// server, and while another server's Talk is live that one keeps gaming:
    /// this page's state is ignored, not mixed in.
    pub(super) fn accept_sync(
        &mut self,
        server: &str,
        token: &str,
        origin: Option<&str>,
        voice: Voice,
    ) -> Result<(), String> {
        let token_ok = !token.is_empty() && self.tokens.get(server).is_some_and(|t| t == token);
        if !token_ok || origin.is_none() || origin != self.origin.as_deref() {
            return Err("Wrong gaming session.".into());
        }
        voice.validate()?;
        let held_elsewhere = self.owner.as_deref().is_some_and(|o| o != server)
            && self.voice.connected
            && self.updated.elapsed() < Duration::from_secs(2);
        if held_elsewhere || (!voice.connected && self.owner.as_deref() != Some(server)) {
            return Ok(());
        }
        if !voice.connected
            || voice.account != self.voice.account
            || voice.channel != self.voice.channel
            || voice.scope != self.voice.scope
        {
            self.overlay = false;
            self.active = false;
            self.previous = [true; 4];
            self.ptt_down = false;
            self.ptt_armed = false;
        }
        self.owner = voice.connected.then(|| server.to_owned());
        self.voice = voice;
        self.updated = Instant::now();
        Ok(())
    }
}
/// Hands a server's page the token that lets it feed gaming.
pub fn bind(app: &tauri::AppHandle, server: &str) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.origin = crate::web_client::local_origin(&crate::web_client::app_url());
        g.tokens.insert(server.to_owned(), token.clone());
        if g.owner.as_deref() == Some(server) {
            g.release();
        }
    }
    token
}
impl Gaming {
    /// Tells the key tracker the chords the settings now hold.
    pub(super) fn sync_chords(&self) {
        if let Ok(mut keys) = self.keys.lock() {
            keys.set_chords(self.settings.chords());
        }
    }
    /// Nobody owns the Talk: no overlay, nothing held.
    pub(super) fn release(&mut self) {
        let keep = (self.origin.clone(), std::mem::take(&mut self.tokens));
        *self = Gaming {
            settings: self.settings.clone(),
            origin: keep.0,
            tokens: keep.1,
            foreground: self.foreground.take(),
            candidate: self.candidate.take(),
            detection_note: self.detection_note,
            library: self.library.clone(),
            keys: self.keys.clone(),
            hint: std::mem::take(&mut self.hint),
            ..Gaming::default()
        };
    }
}
/// A server's page is gone (its server left the list): it no longer has a
/// token, and gaming lets go of it if it owned the Talk.
pub fn forget(app: &tauri::AppHandle, server: &str) {
    let owned = app.state::<Mutex<Gaming>>().lock().is_ok_and(|mut g| {
        g.tokens.remove(server);
        let owned = g.owner.as_deref() == Some(server);
        if owned {
            g.release();
        }
        owned
    });
    if owned {
        close_surfaces(app);
    }
}
/// The window is closing: everything lets go.
pub fn clear(app: &tauri::AppHandle) {
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.tokens.clear();
        g.release();
    }
    close_surfaces(app);
}
/// How long without an evaluation counts as a stall. The heartbeat runs every
/// half second while a Talk is live, so this is only ever exceeded after sleep,
/// a frozen host or an idle period.
const STALL: Duration = Duration::from_secs(5);
impl Gaming {
    // Pure state transition: native UI effects and command dispatch happen only
    // after the caller releases the shared state lock.
    pub(super) fn advance(&mut self, now: Instant, game: bool, keys: [bool; 4]) -> Vec<Command> {
        let mut commands = Vec::new();
        // Nothing looked at the keys for a long while (sleep, a stalled window
        // thread, an idle period): a held key says nothing about a new press.
        if now.duration_since(self.last_tick) > STALL {
            self.previous = keys;
            self.ptt_armed = false;
        }
        self.last_tick = now;
        let active = self.settings.enabled
            && game
            && self.origin.is_some()
            && self.voice.connected
            && self.updated.elapsed() < Duration::from_secs(2);
        if !active {
            self.overlay = false;
            self.previous = keys;
            if self.ptt_down {
                commands.push(Command::new(self, "ptt-release"));
            }
            self.ptt_down = false;
            self.ptt_armed = false;
        } else {
            if keys[0] && !self.previous[0] {
                self.overlay = !self.overlay;
            }
            for (i, action) in [(1, "mute"), (2, "deafen")] {
                if keys[i] && !self.previous[i] {
                    commands.push(Command::new(self, action));
                }
            }
            if !keys[3] {
                self.ptt_armed = true;
            }
            if self.voice.ptt_mode && !self.overlay && self.ptt_armed && keys[3] {
                commands.push(Command::new(self, "ptt-press"));
                self.ptt_down = true;
            } else if self.ptt_down {
                commands.push(Command::new(self, "ptt-release"));
                self.ptt_down = false;
            }
            self.previous = keys;
        }
        self.active = active;
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::super::voice::Member;
    use super::*;

    fn ptt_talk() -> Gaming {
        let mut g = Gaming {
            origin: Some("https://example.invalid".into()),
            ..Gaming::default()
        };
        g.voice.connected = true;
        g.voice.ptt_mode = true;
        g.sync_chords();
        g
    }
    fn look(g: &mut Gaming, now: Instant, game: bool) -> (Vec<&'static str>, bool) {
        let (keys, again) = g.keys.lock().unwrap().sample();
        let actions = g.advance(now, game, keys);
        (
            actions
                .iter()
                .map(|c| c.action.as_str())
                .map(leak)
                .collect(),
            again,
        )
    }
    fn leak(text: &str) -> &'static str {
        match text {
            "ptt-press" => "ptt-press",
            "ptt-release" => "ptt-release",
            "mute" => "mute",
            "deafen" => "deafen",
            other => panic!("unexpected action {other}"),
        }
    }
    fn press(g: &Gaming, key: u16, down: bool) {
        g.keys
            .lock()
            .unwrap()
            .apply(input::InputEvent::Key(input::KeyEvent { key, down }));
    }

    #[test]
    fn held_push_to_talk_key_presses_then_releases_from_key_events_alone() {
        let mut g = ptt_talk();
        let now = Instant::now();
        assert_eq!(look(&mut g, now, true), (vec![], false));
        press(&g, 0x20, true);
        assert_eq!(look(&mut g, now, true).0, vec!["ptt-press"]);
        // The page lets a press lapse after 150 ms, so a held key is re-sent.
        assert_eq!(look(&mut g, now, true).0, vec!["ptt-press"], "keepalive");
        press(&g, 0x20, false);
        assert_eq!(look(&mut g, now, true).0, vec!["ptt-release"]);
    }
    #[test]
    fn a_tap_between_two_looks_still_presses_and_then_releases() {
        let mut g = ptt_talk();
        let now = Instant::now();
        look(&mut g, now, true);
        press(&g, 0x20, true);
        press(&g, 0x20, false);
        let (first, again) = look(&mut g, now, true);
        assert_eq!(first, vec!["ptt-press"]);
        assert!(again, "the release must still be looked at");
        assert_eq!(look(&mut g, now, true), (vec!["ptt-release"], false));
    }
    #[test]
    fn keys_outside_a_game_do_nothing_and_a_key_held_into_a_game_waits_for_a_release() {
        let mut g = ptt_talk();
        let now = Instant::now();
        look(&mut g, now, false);
        press(&g, 0x20, true);
        assert_eq!(look(&mut g, now, false).0, Vec::<&str>::new());
        // The game comes to the front while the key is still down.
        assert_eq!(look(&mut g, now, true).0, Vec::<&str>::new());
        press(&g, 0x20, false);
        look(&mut g, now, true);
        press(&g, 0x20, true);
        assert_eq!(look(&mut g, now, true).0, vec!["ptt-press"]);
    }
    #[test]
    fn leaving_the_game_or_a_lost_key_up_releases_push_to_talk() {
        let mut g = ptt_talk();
        let now = Instant::now();
        look(&mut g, now, true);
        press(&g, 0x20, true);
        look(&mut g, now, true);
        // Focus leaves the game while the key is down.
        assert_eq!(look(&mut g, now, false).0, vec!["ptt-release"]);
        // The session locked: Raw Input could not report the key-up, the source resets.
        let mut g = ptt_talk();
        look(&mut g, now, true);
        press(&g, 0x20, true);
        look(&mut g, now, true);
        g.keys.lock().unwrap().apply(input::InputEvent::Reset);
        assert_eq!(look(&mut g, now, true).0, vec!["ptt-release"]);
    }
    #[test]
    fn a_long_gap_between_looks_rearms_instead_of_pressing() {
        let mut g = ptt_talk();
        let start = Instant::now();
        look(&mut g, start, true);
        press(&g, 0x20, true);
        let later = start + STALL + Duration::from_millis(1);
        assert_eq!(look(&mut g, later, true).0, Vec::<&str>::new());
    }
    #[test]
    fn mute_and_deafen_chords_fire_once_per_press() {
        let mut g = ptt_talk();
        let now = Instant::now();
        look(&mut g, now, true);
        press(&g, 0x12, true);
        press(&g, 0x4e, true);
        assert_eq!(look(&mut g, now, true).0, vec!["mute"]);
        assert_eq!(look(&mut g, now, true).0, Vec::<&str>::new());
        press(&g, 0x4e, false);
        press(&g, 0x44, true);
        assert_eq!(look(&mut g, now, true).0, vec!["deafen"]);
    }
    #[test]
    fn global_disable_hides_surfaces_and_releases_ptt_without_reopening_held_key() {
        let mut g = Gaming {
            origin: Some("https://example.invalid".into()),
            ..Gaming::default()
        };
        g.voice.connected = true;
        g.voice.ptt_mode = true;
        let now = Instant::now();
        g.advance(now, true, [false; 4]);
        g.advance(now, true, [false, false, false, true]);
        assert!(g.ptt_down);
        g.overlay = true;
        g.settings.enabled = false;
        let commands = g.advance(now, true, [true; 4]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].action, "ptt-release");
        assert!(!g.active);
        assert!(!g.overlay);
        g.settings.enabled = true;
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.ptt_down);
        assert!(!g.overlay);
    }

    #[test]
    fn chords_act_only_on_new_key_edges_while_gaming_is_active() {
        let mut g = ptt_talk();
        let now = Instant::now();
        assert!(g.advance(now, false, [false; 4]).is_empty());
        assert!(!g.active);
        assert!(g.advance(now, true, [false; 4]).is_empty());
        assert!(g.active);
        let actions = g.advance(now, true, [true, true, true, false]);
        assert!(g.overlay);
        assert_eq!(
            actions
                .iter()
                .map(|c| c.action.as_str())
                .collect::<Vec<_>>(),
            ["mute", "deafen"]
        );
        assert!(g.advance(now, true, [true, true, true, false]).is_empty());
        g.advance(now, true, [false; 4]);
        g.advance(now, true, [true, false, false, false]);
        assert!(!g.overlay);
    }

    #[test]
    fn leaving_the_game_releases_a_pressed_push_to_talk() {
        let mut g = ptt_talk();
        let now = Instant::now();
        g.advance(now, true, [false; 4]);
        let actions = g.advance(now, true, [false, false, false, true]);
        assert_eq!(actions[0].action, "ptt-press");
        let actions = g.advance(now, false, [false; 4]);
        assert_eq!(actions[0].action, "ptt-release");
        assert!(!g.active);
        assert!(!g.overlay);
    }

    #[test]
    fn a_stale_report_or_a_gap_between_looks_ends_gaming() {
        let mut g = ptt_talk();
        let now = Instant::now();
        g.advance(now, true, [false; 4]);
        g.updated = now - Duration::from_secs(3);
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.active);
        g.updated = Instant::now();
        g.last_tick = now - Duration::from_secs(1);
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.ptt_armed);
    }

    fn talk(account: &str, channel: &str, scope: &str) -> Voice {
        Voice {
            connected: true,
            scope: Some(scope.into()),
            account: Some(account.into()),
            channel: Some(channel.into()),
            members: vec![Member {
                id: account.into(),
                name: "Player".into(),
                speaking: false,
                muted: false,
                sharing: false,
            }],
            ..Voice::default()
        }
    }

    fn two_servers() -> Gaming {
        let mut g = Gaming {
            origin: Some("mnema://localhost".into()),
            ..Gaming::default()
        };
        g.tokens.insert("a".into(), "token-a".into());
        g.tokens.insert("b".into(), "token-b".into());
        g
    }

    const U1: &str = "11111111-1111-4111-8111-111111111111";
    const U2: &str = "22222222-2222-4222-8222-222222222222";
    const U3: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn a_page_needs_the_token_of_its_own_server_and_the_apps_origin() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        assert!(
            g.accept_sync("a", "token-b", origin, talk(U1, U2, U3))
                .is_err()
        );
        assert!(g.accept_sync("a", "", origin, talk(U1, U2, U3)).is_err());
        assert!(
            g.accept_sync("c", "token-a", origin, talk(U1, U2, U3))
                .is_err()
        );
        assert!(
            g.accept_sync(
                "a",
                "token-a",
                Some("https://evil.example"),
                talk(U1, U2, U3)
            )
            .is_err()
        );
        assert!(
            g.accept_sync("a", "token-a", None, talk(U1, U2, U3))
                .is_err()
        );
        assert!(g.owner.is_none(), "nothing was accepted");
        assert!(
            g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
                .is_ok()
        );
        assert_eq!(g.owner.as_deref(), Some("a"));
    }

    #[test]
    fn gaming_stays_with_the_server_that_owns_the_talk() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
            .unwrap();
        // B reports its own Talk while A's is live: ignored, not mixed in.
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("a"));
        assert_eq!(g.voice.account.as_deref(), Some(U1));
        // B saying "not connected" does not end A's Talk either.
        g.accept_sync("b", "token-b", origin, Voice::default())
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("a"));
        assert!(g.voice.connected);
        // Commands go back to A's page with A's token.
        let command = Command::new(&g, "mute");
        assert_eq!(
            (command.owner.as_deref(), command.token.as_str()),
            (Some("a"), "token-a")
        );
        // A leaves its Talk: gaming lets go, and B may take it.
        g.accept_sync("a", "token-a", origin, Voice::default())
            .unwrap();
        assert_eq!(g.owner, None);
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("b"));
        let command = Command::new(&g, "mute");
        assert_eq!(
            (command.owner.as_deref(), command.token.as_str()),
            (Some("b"), "token-b")
        );
    }

    #[test]
    fn a_stale_owner_does_not_hold_gaming_forever() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
            .unwrap();
        g.updated = Instant::now() - Duration::from_secs(3);
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(
            g.owner.as_deref(),
            Some("b"),
            "A stopped reporting, B takes over"
        );
        assert!(!g.overlay && !g.active, "a different Talk starts clean");
    }

    #[test]
    fn letting_go_of_a_server_forgets_its_token_and_keeps_the_settings() {
        let mut g = two_servers();
        g.settings.sidepeek_opacity = 20;
        g.accept_sync("a", "token-a", Some("mnema://localhost"), talk(U1, U2, U3))
            .unwrap();
        g.release();
        assert_eq!(g.owner, None);
        assert!(!g.voice.connected);
        assert_eq!(g.settings.sidepeek_opacity, 20);
        assert_eq!(g.tokens.len(), 2, "release keeps the tokens of live pages");
        assert_eq!(g.origin.as_deref(), Some("mnema://localhost"));
    }
}
