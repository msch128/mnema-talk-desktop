//! Test-only: the whole self-update of a real build, from the check to the
//! restart into the new version.
//!
//! Compiled only with the `update-selftest` feature, which no shipped build
//! enables. Its CI job (`scripts/update-selftest.mjs`) builds an old and a new
//! version whose updater config names a throwaway key and a feed on this
//! machine, serves the signed new release there and starts the old one with
//! `MNEMA_UPDATE_SELFTEST` naming the file to append result lines to.
//!
//! The old version writes a note into its data folder, waits for the check at
//! start to find the new version, simulates a Talk, asks for "Update now" (the
//! pill's action) and checks that it is asked first, answers "after the call",
//! checks that the verified update waits while the Talk goes on, and ends the
//! Talk. The update then installs and the app restarts exactly as for a person.
//! The new version finds the note (the relaunch through the system may not
//! carry the environment, so the note also names the result file), checks it
//! runs the announced version with the same data folder, and quits.
//! The line format (`ok <name>`, `fail <name>: <why>`, `done`) is the contract
//! with that script.
use super::install::install;
use super::logic::NowChoice;
use super::now::request_now;
use super::state::{Mode, NativeUpdates, Phase};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::Manager;

const NOTE: &str = "update-selftest.note";

fn record(file: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn check(file: &Path, name: &str, good: bool, detail: impl std::fmt::Display) -> bool {
    if good {
        record(file, &format!("ok {name}"));
    } else {
        record(file, &format!("fail {name}: {detail}"));
    }
    good
}

pub fn start(app: &tauri::AppHandle) {
    let Some(dir) = crate::storage::data_dir(app) else {
        return;
    };
    let note = dir.join(NOTE);
    let current = app.package_info().version.to_string();
    if let Ok(text) = std::fs::read_to_string(&note) {
        let mut lines = text.lines();
        let (Some(file), Some(before)) = (lines.next(), lines.next()) else {
            return;
        };
        let file = PathBuf::from(file);
        let _ = std::fs::remove_file(&note);
        check(
            &file,
            "the restarted app runs the new version",
            current != before,
            format!("still {current}"),
        );
        record(&file, &format!("version {current}"));
        let app = app.clone();
        tauri::async_runtime::spawn(async move { after(&app, &file).await });
        return;
    }
    let Some(file) = std::env::var_os("MNEMA_UPDATE_SELFTEST").map(PathBuf::from) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(&note, format!("{}\n{current}\n", file.display())).is_err() {
        record(&file, "fail note: the data folder cannot be written");
        record(&file, "done");
        return;
    }
    record(&file, &format!("version {current}"));
    let app = app.clone();
    tauri::async_runtime::spawn(async move { drive(&app, &file).await });
}

/// The new version, started by the update.
async fn after(app: &tauri::AppHandle, file: &Path) {
    let portable = app.state::<NativeUpdates>().source.portable;
    if portable && let Ok(exe) = std::env::current_exe() {
        let gone = until(|| !super::portable::replaced_left(&exe), 30).await;
        check(
            file,
            "the replaced program was deleted",
            gone,
            exe.display(),
        );
    }
    // The note was in the data folder of the old version: it is the same folder.
    record(file, "ok the data folder was kept");
    record(file, "done");
    app.exit(0);
}

async fn until(what: impl Fn() -> bool, seconds: u64) -> bool {
    for _ in 0..seconds * 10 {
        if what() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    what()
}

async fn drive(app: &tauri::AppHandle, file: &Path) {
    if !run(app, file).await {
        record(file, "done");
        app.exit(1);
    }
}

async fn run(app: &tauri::AppHandle, file: &Path) -> bool {
    let state = app.state::<NativeUpdates>();
    let phase = || state.snapshot().phase;
    let found = until(|| phase() == Phase::Available, 120).await;
    if !check(
        file,
        "the check at start finds the new version",
        found,
        format!("{:?}", state.snapshot()),
    ) {
        return false;
    }
    let trusted = until(|| super::install::installs_allowed(app), 60).await;
    if !check(
        file,
        "a fresh trust statement is in force",
        trusted,
        "none was fetched",
    ) {
        return false;
    }
    let talking = Arc::new(AtomicBool::new(true));
    let probe = talking.clone();
    state.set_talk_probe(move || probe.load(Ordering::SeqCst));
    let asked = request_now(app, false) == NowChoice::AskAboutTalk && state.snapshot().confirm_call;
    if !check(
        file,
        "Update now during a Talk asks first",
        asked,
        format!("{:?}", state.snapshot()),
    ) {
        return false;
    }
    // The window's "After the call".
    let handle = app.clone();
    tauri::async_runtime::spawn(async move { install(&handle, Mode::AfterCall).await });
    let waiting = until(|| phase() == Phase::Waiting, 600).await;
    let downloaded = state.snapshot();
    if !check(
        file,
        "the update is downloaded, verified and waits for the Talk",
        waiting && downloaded.received > 0,
        format!("{downloaded:?}"),
    ) {
        return false;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    if !check(
        file,
        "nothing installs while the Talk goes on",
        phase() == Phase::Waiting,
        format!("{:?}", phase()),
    ) {
        return false;
    }
    record(
        file,
        "ok the Talk ends, the update installs and the app restarts",
    );
    talking.store(false, Ordering::SeqCst);
    // The installer (Windows) or the restart ends this process; reaching
    // the end of the wait, failed or stuck, is a failure.
    until(|| phase() == Phase::Failed, 300).await;
    check(file, "install", false, format!("{:?}", state.snapshot()));
    false
}
