//! The UI-thread watchdog. A thread of its own asks the UI thread to answer a
//! message every 100 ms; when the answer takes 200 ms or more it writes down what
//! the UI thread was doing (the steps the host marks with `ui_guard::doing`).
//! The file is `logs/desktop.log` in the data folder: beside the program in
//! `mnema-data` for a portable copy, in the profile's local app data otherwise.
#[cfg(windows)]
#[path = "../../../src/hung_window.rs"]
pub mod hung_window;
#[allow(dead_code)]
#[path = "../../../src/ui_guard.rs"]
pub mod ui_guard;

/// Opens the log and starts the watchdog. Call on the UI thread, once.
pub fn start(app: &tauri::AppHandle) {
    if let Some(dir) = super::storage::data_dir(app) {
        ui_guard::init_log(&dir.join("logs"));
    }
    ui_guard::log(&format!(
        "started {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    ));
    #[cfg(windows)]
    watch(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });
}

#[cfg(windows)]
fn watch(ui_thread: u32) {
    use std::time::{Duration, Instant};
    let _ = std::thread::Builder::new()
        .name("mnema-ui-watch".into())
        .spawn(move || {
            let mut tracker = ui_guard::StallTracker::default();
            let mut window = None;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let Some(hwnd) = window.or_else(|| hung_window::window_of_thread(ui_thread)) else {
                    continue;
                };
                window = Some(hwnd);
                let begin = Instant::now();
                let answered = hung_window::answers(hwnd, ui_guard::STALL);
                let doing = ui_guard::current().map(|(label, _)| label);
                if let Some(report) =
                    tracker.probe(Instant::now(), answered, begin.elapsed(), doing)
                {
                    ui_guard::log(&report.line());
                }
                if !answered {
                    // The window may be gone (a closed one); look it up again.
                    window = None;
                }
            }
        });
}
