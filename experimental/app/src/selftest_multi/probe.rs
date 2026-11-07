//! The helpers of the self-test: the result record and the page probes.
use crate::web_client::Trusted;
use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use tauri::Manager;

pub(super) fn record(line: &str) {
    crate::web_permissions::append_log_line("MNEMA_MULTI_SELFTEST", line);
}

/// How many checks ran, so the script compares the passes with the host's own count.
pub(super) static CHECKS: AtomicU32 = AtomicU32::new(0);

pub(super) fn check(name: &str, good: bool, detail: impl std::fmt::Display) {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if good {
        record(&format!("ok {name}"));
    } else {
        record(&format!("fail {name}: {detail}"));
    }
}

pub(super) use crate::selftest_page::{eval, run};

/// The same, for a page that was just made: tries until the page answers.
pub(super) async fn run_when_ready(app: &tauri::AppHandle, label: &str, body: &str) -> String {
    for _ in 0..100 {
        let result = run(app, label, body).await;
        if result != "no page" {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    "no page".into()
}

pub(super) async fn until(what: &str, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..300 {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    record(&format!("fail {what}: timed out"));
    false
}

pub(super) fn counts(app: &tauri::AppHandle, id: &str) -> Option<(u32, u32)> {
    let trusted = app.state::<Trusted>();
    let registry = trusted.registry.lock().ok()?;
    let rail = registry.rail(true);
    rail.instances
        .iter()
        .find(|i| i.id == id)
        .map(|i| (i.unread, i.mentions))
}

pub(super) fn front(app: &tauri::AppHandle) -> Option<String> {
    let trusted = app.state::<Trusted>();
    let registry = trusted.registry.lock().ok()?;
    registry.active().map(str::to_owned)
}

pub(super) fn badge(app: &tauri::AppHandle) -> u32 {
    let trusted = app.state::<Trusted>();
    trusted.registry.lock().map_or(u32::MAX, |r| r.badge())
}
