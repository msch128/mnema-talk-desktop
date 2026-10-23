//! Test-only: posts one real toast without a page and records what happened.
//!
//! Compiled only with the `notify-selftest` feature, which no shipped build
//! enables, and even then inert unless `MNEMA_NOTIFY_SELFTEST` names the file
//! to append the result lines to. CI uses it to prove, on the real installed
//! app, that a toast reaches the operating system; the line format is the
//! contract with `scripts/test-windows-notify.ps1` and the macOS check.
use super::{Click, apply_badge, deliver, platform};
use std::{io::Write, time::Duration};
use tauri::Manager;

pub const CHANNEL: &str = "00000000-0000-4000-8000-000000000001";
pub const MESSAGE: &str = "00000000-0000-4000-8000-000000000002";

fn record(line: &str) {
    let Some(path) = std::env::var_os("MNEMA_NOTIFY_SELFTEST") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

static CLICKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A toast was used (called from `activated`).
pub fn clicked(click: &Click) {
    CLICKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    record(&format!(
        "clicked channel={} message={}",
        click.channel_id, click.message_id
    ));
}

/// A server of its own for the toasts, so that they are tied to one like the
/// page's are; its name never leaves this test.
fn selftest_server(app: &tauri::AppHandle) -> Option<(String, String)> {
    use crate::instances::AddError;
    let trusted = app.state::<crate::web_client::Trusted>();
    let mut registry = trusted.registry.lock().ok()?;
    let id = match registry.add(url::Url::parse("https://selftest.invalid/").ok()?) {
        Ok(id) | Err(AddError::Known(id)) => id,
        Err(AddError::Full) => return None,
    };
    let generation = registry.slot(&id)?.generation.clone();
    Some((id, generation))
}

pub fn start(app: &tauri::AppHandle) {
    if std::env::var_os("MNEMA_NOTIFY_SELFTEST").is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The windows are up and the platform side is initialised.
        tokio::time::sleep(Duration::from_secs(3)).await;
        // Windows: no banner can be pressed on a runner, so call the activation
        // handler the way the WinRT event does: the current session must be
        // reported, a stale one (an earlier instance) must be dropped.
        #[cfg(windows)]
        {
            use std::sync::atomic::Ordering::SeqCst;
            let click = Click {
                channel_id: CHANNEL.into(),
                message_id: MESSAGE.into(),
            };
            let Some((_, session)) = selftest_server(&app) else {
                record("synthetic-activation reported=false stale-dropped=false");
                return;
            };
            let before = CLICKS.load(SeqCst);
            platform::simulate_activation(&app, click.clone(), &session);
            let reported = CLICKS.load(SeqCst) == before + 1;
            platform::simulate_activation(&app, click, "stale");
            let dropped = CLICKS.load(SeqCst) == before + 1;
            record(&format!(
                "synthetic-activation reported={reported} stale-dropped={dropped}"
            ));
        }
        // The badge too, on the window: the app-wide count is read back from outside.
        if let Some(window) = app.get_window("shell") {
            record(&format!("badge={:?}", apply_badge(&window, 3)));
        }
        let Some((server, _)) = selftest_server(&app) else {
            record("shown=error no server");
            return;
        };
        record(&format!("permission={:?}", platform::permission().await));
        let result = deliver(
            &app,
            &server,
            "Mnema selftest",
            "A test toast from the installed app",
            CHANNEL.into(),
            MESSAGE.into(),
        )
        .await;
        match result {
            Ok(true) => record("shown=ok"),
            Ok(false) => record("shown=dropped"),
            Err(e) => record(&format!("shown=error {e}")),
        }
        let wait = std::env::var("MNEMA_NOTIFY_SELFTEST_WAIT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10);
        tokio::time::sleep(Duration::from_secs(wait)).await;
        record("done");
        app.exit(0);
    });
}
