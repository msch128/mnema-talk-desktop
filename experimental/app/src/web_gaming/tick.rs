//! The evaluation of gaming: a wake-up collapses into one pass on the main
//! thread, and a heartbeat covers what expires without any event.
use super::native::native_sample;
use super::state::Gaming;
use super::surface::surface;
use super::{fullscreen, input_host::manage_input};
use crate::ui_watch::ui_guard;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::Manager;

fn tick(app: &tauri::AppHandle) {
    let _step = ui_guard::doing("gaming evaluation");
    let shared = app.state::<Mutex<Gaming>>();
    let Ok(mut g) = shared.lock() else {
        return;
    };
    let now = Instant::now();
    let fresh =
        g.settings.enabled && g.voice.connected && g.updated.elapsed() < Duration::from_secs(2);
    // The shortcuts are listened for in any Talk with gaming on.
    let want_input = fresh;
    let tracker = g.keys.clone();
    let (game, position) = if fresh {
        let _step = ui_guard::doing("foreground window sample");
        native_sample(&mut g, app)
    } else {
        (false, None)
    };
    let (keys, again) = if fresh {
        tracker
            .lock()
            .map(|mut t| t.sample())
            .unwrap_or(([false; 4], false))
    } else {
        ([false; 4], false)
    };
    let commands = g.advance(now, game, keys);
    let active = g.active;
    let overlay = g.overlay;
    let holding = g.ptt_down;
    // The shell's answer is asked for on a worker thread, and only while a game
    // is in front and the hint is still due.
    let probe = cfg!(windows) && active && !g.hint.done();
    drop(g);
    if probe {
        look_at_screen(app);
    }
    app.state::<Heartbeat>().set(match (fresh, holding) {
        (false, _) => None,
        (true, false) => Some(Duration::from_millis(500)),
        (true, true) => Some(Duration::from_millis(100)),
    });
    manage_input(app, &tracker, want_input);
    if again {
        // A press that began and ended between two looks is still to be released.
        wake(app);
    }
    for command in commands {
        command.dispatch(app);
    }
    // Only Windows draws the sidepeek and the overlay.
    if !cfg!(windows) {
        return;
    }
    let _step = ui_guard::doing("sidepeek and overlay windows");
    for (label, visible) in [("sidepeek", active), ("gaming-overlay", active && overlay)] {
        if visible {
            let made = {
                let _step = ui_guard::doing("create a gaming window");
                surface(app, label, label == "gaming-overlay")
            };
            if let Ok(w) = made {
                // A window that is already there is not moved or shown again:
                // every call is a message into the window system over a game.
                if let Some(p) = position
                    && w.outer_position().map_or(true, |now| now != p)
                {
                    let _ = w.set_position(p);
                }
                if !w.is_visible().unwrap_or(false) {
                    let _ = w.show();
                    if label == "gaming-overlay" {
                        let _ = w.set_focus();
                    }
                }
            }
        } else if let Some(w) = app.get_webview_window(label)
            && w.is_visible().unwrap_or(false)
        {
            let _ = w.hide();
        }
    }
}
/// Windows' own answer to "is a Direct3D app running in exclusive fullscreen?".
#[cfg(windows)]
fn screen_state() -> fullscreen::ScreenState {
    use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
    let mut code = 0;
    // SAFETY: one out-parameter that lives across the call.
    let status = unsafe { SHQueryUserNotificationState(&mut code) };
    if status == 0 {
        fullscreen::from_quns(code)
    } else {
        fullscreen::ScreenState::Normal
    }
}
#[cfg(not(windows))]
fn screen_state() -> fullscreen::ScreenState {
    fullscreen::ScreenState::Normal
}
/// How often the shell is asked whether a game owns the display.
const SCREEN_LOOK_EVERY: Duration = Duration::from_secs(2);

/// Asks the shell about exclusive fullscreen on a thread of its own. The shell
/// answers over RPC to the desktop, which can take seconds while a game is
/// starting; the UI thread must never wait for it. At most one question is out at
/// a time and they are spaced, so a stuck shell costs one idle thread. The toast
/// and the marker file follow on the same thread.
fn look_at_screen(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    static SPACING: Mutex<ui_guard::Throttle> =
        Mutex::new(ui_guard::Throttle::new(SCREEN_LOOK_EVERY));
    if !SPACING
        .lock()
        .is_ok_and(|mut spacing| spacing.ready(Instant::now()))
        || BUSY.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("mnema-screen-state".into())
        .spawn(move || {
            let state = screen_state();
            let due = app.state::<Mutex<Gaming>>().lock().is_ok_and(|mut g| {
                let active = g.active;
                let due = g.hint.look(active, state);
                g.exclusive_hint |= due;
                due
            });
            BUSY.store(false, Ordering::Release);
            if due {
                announce_exclusive_fullscreen(&app);
            }
        });
    if spawned.is_err() {
        BUSY.store(false, Ordering::Release);
    }
}

/// Tells the person once. The toast waits in the notification centre while the
/// game owns the display, and the snapshot carries the same hint for the
/// surfaces and the settings.
fn announce_exclusive_fullscreen(app: &tauri::AppHandle) {
    crate::web_notify::host_toast(app, fullscreen::HINT_TITLE, fullscreen::HINT_BODY);
    match crate::storage::config_dir(app).ok_or("no profile") {
        Ok(dir) => {
            if let Err(why) = fullscreen::remember_shown(&dir) {
                eprintln!("Mnema Desktop: could not remember the fullscreen hint: {why}");
            }
        }
        Err(why) => {
            eprintln!("Mnema Desktop: no config folder to remember the fullscreen hint: {why}")
        }
    }
}
/// Evaluate the gaming state on the main thread soon. Many wake-ups collapse
/// into one evaluation; one that arrives while an evaluation runs queues another.
static WAKE_QUEUED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn wake(app: &tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if WAKE_QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let next = app.clone();
    if app
        .run_on_main_thread(move || {
            WAKE_QUEUED.store(false, Ordering::Release);
            tick(&next);
        })
        .is_err()
    {
        WAKE_QUEUED.store(false, Ordering::Release);
    }
}
/// A watchdog for the things that expire without any event: a Talk page that
/// stopped reporting, a game process that exited, a permission granted in the
/// system settings. It does not look at keys or at the foreground. It sleeps
/// until a Talk is live, then wakes twice a second.
///
/// While push-to-talk is held it wakes every 100 ms for one reason only: the
/// page honours a `ptt-press` for 150 ms and drops the press 300 ms after the
/// last one (a dead-man switch against a stuck microphone), so the host repeats
/// the press it already knows about from the key event.
#[derive(Default)]
pub(super) struct Heartbeat {
    period: Mutex<Option<Duration>>,
    changed: std::sync::Condvar,
}
impl Heartbeat {
    pub(super) fn set(&self, period: Option<Duration>) {
        if let Ok(mut now) = self.period.lock()
            && *now != period
        {
            *now = period;
            self.changed.notify_all();
        }
    }
    pub(super) fn run(&self, app: &tauri::AppHandle) {
        loop {
            let Ok(mut period) = self.period.lock() else {
                return;
            };
            let every = loop {
                match *period {
                    Some(every) => break every,
                    None => {
                        period = match self.changed.wait(period) {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };
                    }
                }
            };
            drop(self.changed.wait_timeout(period, every));
            wake(app);
        }
    }
}
