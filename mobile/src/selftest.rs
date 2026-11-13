//! Test-only (`mobile-selftest`): the app records how far it gets, so CI can
//! read it from an emulator or a simulator without driving the screen.
//!
//! 1. `tls`: an HTTPS request to a public host through the shared client, which
//!    shows that the system's certificate check runs (on Android through JNI).
//! 2. `picker`: the app shell of the verified bundle was served in the picker
//!    role.
//! 3. `opened`: the server named at build time (`MNEMA_SELFTEST_SERVER`) was
//!    asked what it is, matched with the bundle and opened, as the picker's
//!    command does.
//! 4. `proxied`: the page of that server sent its first `/api/` request
//!    through the proxy and got an answer from the server.
//!
//! Each step is a line of `selftest.jsonl` in the app's data directory and on
//! stderr, with the milliseconds since the app started. Every load of the
//! identity script is recorded too (`identity`, with the role), so a
//! screenshot can be matched to the role the page ran in.
use crate::web_proxy::Resp;
use std::{
    io::Write,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::Manager;

const SERVER: &str = match option_env!("MNEMA_SELFTEST_SERVER") {
    Some(server) => server,
    None => "http://127.0.0.1:8765",
};

/// How long the picker stays on screen, counted from the page loading its
/// identity as the picker, before the server is opened: a cold emulator or
/// simulator needs seconds to run the bundle's scripts, and CI takes its
/// screenshot 10 seconds in.
const PICKER_TIME: Duration = Duration::from_secs(25);

struct Progress {
    /// The picker's document was served.
    picker: bool,
    /// The page loaded its identity as the picker.
    selector: bool,
    proxied: bool,
}

static PROGRESS: Mutex<Progress> = Mutex::new(Progress {
    picker: false,
    selector: false,
    proxied: false,
});

static STARTED: OnceLock<Instant> = OnceLock::new();

fn record(app: &tauri::AppHandle, step: &str, detail: &str) {
    let ms = STARTED.get_or_init(Instant::now).elapsed().as_millis();
    let line = serde_json::json!({ "step": step, "detail": detail, "ms": ms }).to_string();
    eprintln!("mnema-selftest {line}");
    let Ok(dir) = app.path().app_local_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("selftest.jsonl"))
    {
        let _ = writeln!(file, "{line}");
    }
}

pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    STARTED.get_or_init(Instant::now);
    tauri::async_runtime::spawn(async move {
        record(&app, "started", SERVER);
        record(&app, "tls", &tls_probe().await);
        for _ in 0..240 {
            if PROGRESS.lock().is_ok_and(|p| p.selector) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(PICKER_TIME).await;
        match crate::host::open_server(&app, SERVER).await {
            Ok(()) => record(&app, "opened", SERVER),
            Err(why) => record(&app, "failed", &why),
        }
    });
}

/// Any HTTP answer means the TLS handshake and the certificate check passed.
async fn tls_probe() -> String {
    let Ok(client) = crate::web_proxy::client() else {
        return "error: no client".into();
    };
    match client.get("https://github.com/").send().await {
        Ok(response) => format!("ok {}", response.status().as_u16()),
        Err(why) => format!("error: {why:?}"),
    }
}

/// The protocol handler served a file of the bundle.
pub fn served(app: &tauri::AppHandle, path: &str, resp: &Resp) {
    let shell = resp
        .headers
        .iter()
        .any(|(k, v)| k == "content-type" && v.starts_with("text/html"));
    let picker = app.state::<crate::host::Host>().active().is_none();
    if !(shell && picker) {
        return;
    }
    let first = PROGRESS
        .lock()
        .is_ok_and(|mut p| !std::mem::replace(&mut p.picker, true));
    if first {
        record(app, "picker", &format!("{path} {}", resp.status));
    }
}

/// The proxy answered a request of the server's page.
pub fn proxied(app: &tauri::AppHandle, method: &str, path: &str, status: u16) {
    let first = PROGRESS
        .lock()
        .is_ok_and(|mut p| !std::mem::replace(&mut p.proxied, true));
    if first {
        record(app, "proxied", &format!("{method} {path} {status}"));
    }
}

/// The page loaded the identity script for a role.
pub fn identity(app: &tauri::AppHandle, role: &str) {
    if role == "selector"
        && let Ok(mut progress) = PROGRESS.lock()
    {
        progress.selector = true;
    }
    record(app, "identity", role);
}
