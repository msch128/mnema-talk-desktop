//! The key source, started only while a Talk is live and gaming is on.
use super::input;
use super::state::Gaming;
use super::tick::wake;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::Manager;

/// The key source, started only while a Talk is live and gaming is on.
#[derive(Default)]
pub(super) struct InputHost {
    source: Mutex<Option<Box<dyn input::InputSource + Send>>>,
    running: std::sync::atomic::AtomicBool,
    retry_after: Mutex<Option<Instant>>,
}
fn new_input_source() -> Option<Box<dyn input::InputSource + Send>> {
    #[cfg(windows)]
    return Some(Box::new(super::input_windows::RawInputSource::default()));
    #[cfg(not(windows))]
    None
}
pub(super) fn manage_input(
    app: &tauri::AppHandle,
    tracker: &std::sync::Arc<Mutex<input::ChordTracker>>,
    want: bool,
) {
    use std::sync::atomic::Ordering;
    let host = app.state::<InputHost>();
    let Ok(mut slot) = host.source.lock() else {
        return;
    };
    let running = host.running.load(Ordering::Acquire);
    let mut note = None;
    if want && !running {
        if host
            .retry_after
            .lock()
            .is_ok_and(|t| t.is_some_and(|t| t > Instant::now()))
        {
            return;
        }
        let source = slot.get_or_insert_with(|| {
            new_input_source().unwrap_or_else(|| Box::new(input::UnavailableSource))
        });
        let (keys, waker) = (tracker.clone(), app.clone());
        let started = source.start(Box::new(move |event| {
            if keys.lock().is_ok_and(|mut keys| keys.apply(event)) {
                wake(&waker);
            }
        }));
        match started {
            Ok(()) => host.running.store(true, Ordering::Release),
            Err(error) => {
                note = Some(error.note());
                if let Ok(mut retry) = host.retry_after.lock() {
                    *retry = Some(Instant::now() + Duration::from_secs(30));
                }
            }
        }
    } else if !want && running {
        if let Some(source) = slot.as_mut() {
            source.stop();
        }
        host.running.store(false, Ordering::Release);
        if let Ok(mut keys) = tracker.lock() {
            keys.clear();
        }
    } else if running {
        return;
    }
    drop(slot);
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.input_note = if want { note } else { None };
    }
}
