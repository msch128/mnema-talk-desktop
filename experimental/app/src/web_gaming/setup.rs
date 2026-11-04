//! Starting gaming with the program: saved settings, game detection, the
//! launcher libraries and the heartbeat.
use super::input_host::InputHost;
use super::settings::Settings;
use super::state::Gaming;
use super::tick::{Heartbeat, wake};
use super::{foreground, fullscreen, game_candidate, game_library_scan};
use std::{sync::Mutex, time::Duration};
use tauri::Manager;

/// The running foreground source, kept so it lives as long as the app.
#[allow(dead_code)] // held, never read: dropping it would stop the hook
struct Detection(Mutex<Option<Box<dyn foreground::ForegroundSource + Send>>>);

/// Starts event-driven game detection. Where the system offers no way to learn
/// the focused window the note says so and detection stays off.
fn start_detection(app: &tauri::AppHandle) {
    use foreground::ForegroundSource;
    #[cfg(windows)]
    let mut source: Box<dyn ForegroundSource + Send> =
        Box::new(super::foreground_windows::WinEventSource::default());
    #[cfg(not(windows))]
    let mut source: Box<dyn ForegroundSource + Send> =
        Box::new(foreground::UnsupportedSource(foreground::WINDOWS_ONLY_NOTE));
    let handle = app.clone();
    let started = source.start(Box::new(move |front| {
        if let Ok(mut g) = handle.state::<Mutex<Gaming>>().lock() {
            if let Some(name) = game_candidate::candidate(&front, std::process::id()) {
                g.candidate = Some(name);
            }
            g.foreground = Some(front);
        }
        wake(&handle);
    }));
    if let Err(error) = &started
        && let Ok(mut g) = app.state::<Mutex<Gaming>>().lock()
    {
        g.detection_note = Some(error.note());
    }
    app.manage(Detection(Mutex::new(started.is_ok().then_some(source))));
}
/// How often the launcher libraries are read again; saving the gaming settings
/// also triggers a scan, so a game installed a moment ago is known soon.
const LIBRARY_REFRESH: Duration = Duration::from_secs(15 * 60);

/// Reads the launcher manifests on their own thread, never on the foreground path.
fn start_library_scan(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<Gaming>>();
    let library = state.lock().map(|g| g.library.clone());
    if let Ok(library) = library {
        let _ = game_library_scan::start_refresher(
            library,
            game_library_scan::default_launchers(),
            LIBRARY_REFRESH,
        );
    }
}

pub fn setup(app: &tauri::AppHandle) {
    if let Some(dir) = crate::storage::config_dir(app)
        && let Ok(bytes) = std::fs::read(dir.join("gaming.json"))
        && bytes.len() <= 16384
        && let Ok(settings) = serde_json::from_slice::<Settings>(&bytes)
        && settings.validate().is_ok()
        && let Ok(mut g) = app.state::<Mutex<Gaming>>().lock()
    {
        g.settings = settings;
    }
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.sync_chords();
        if let Some(dir) = crate::storage::config_dir(app) {
            g.hint = fullscreen::HintGate::new(fullscreen::was_shown(&dir));
        }
    }
    // tao registers the process's raw input for its own device events. Our
    // input source needs that registration for itself, so tao releases it.
    #[cfg(windows)]
    app.set_device_event_filter(tauri::DeviceEventFilter::Always);
    app.manage(InputHost::default());
    app.manage(Heartbeat::default());
    start_detection(app);
    start_library_scan(app);
    let beat = app.clone();
    std::thread::spawn(move || beat.state::<Heartbeat>().run(&beat));
    #[cfg(feature = "gaming-selftest")]
    selftest::start(app);
}

/// Test-only: a Talk with gaming on is held up by a thread, and the stand-in
/// game of the Windows check (scripts/test-windows-gaming-hang.ps1) is a known
/// game, so detection, the sidepeek and the overlay run on a CI desktop without
/// a server or a real game.
#[cfg(feature = "gaming-selftest")]
mod selftest {
    use super::*;
    use crate::web_gaming::voice::{Member, Voice};
    use std::time::Instant;

    pub const GAME: &str = "mnema-fakegame.exe";
    const ID: &str = "00000000-0000-4000-8000-000000000001";

    pub fn start(app: &tauri::AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            loop {
                if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
                    g.origin = crate::web_client::local_origin(&crate::web_client::app_url());
                    g.owner = Some("selftest".into());
                    g.voice = Voice {
                        connected: true,
                        scope: Some(ID.into()),
                        account: Some(ID.into()),
                        channel: Some(ID.into()),
                        members: vec![Member {
                            id: ID.into(),
                            name: "Self".into(),
                            speaking: false,
                            muted: false,
                            sharing: false,
                        }],
                        ..Voice::default()
                    };
                    g.updated = Instant::now();
                    g.settings.enabled = true;
                    if !g.settings.games.iter().any(|name| name == GAME) {
                        g.settings.games.push(GAME.into());
                    }
                }
                wake(&app);
                std::thread::sleep(Duration::from_millis(400));
            }
        });
    }
}
