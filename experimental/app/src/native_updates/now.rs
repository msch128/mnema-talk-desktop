//! "Update now": asks first during a Talk, waits for its end when asked to,
//! and installs: the installer (Windows, macOS) or the portable copy's own
//! files, then the app restarts with the same data, so the servers and the
//! sign-ins stay.
use super::check::check;
use super::install::{fail, install, installs_allowed};
use super::logic::{NowChoice, call_over, now_choice};
use super::portable::{PortableError, replace};
use super::state::{Failure, Mode, NativeUpdates, Phase};
use super::window::open_window;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::Update;

/// How often a waiting update looks whether the Talk has ended.
#[cfg(not(test))]
const TALK_POLL: Duration = Duration::from_secs(2);
#[cfg(test)]
const TALK_POLL: Duration = Duration::from_millis(10);

/// "Update now" from the rail, the menu or the update window. `confirmed` is
/// the answer "update now anyway" to the question about a running Talk.
pub fn request_now<R: Runtime>(app: &AppHandle<R>, confirmed: bool) -> NowChoice {
    let state = app.state::<NativeUpdates>();
    let snapshot = state.snapshot();
    let choice = now_choice(
        snapshot.phase,
        snapshot.version.is_some(),
        state.in_talk(),
        confirmed,
    );
    match choice {
        NowChoice::StopWaiting => {
            state.with(|i| i.force_now = true);
        }
        NowChoice::AskAboutTalk => {
            state.with(|i| i.snapshot.confirm_call = true);
            let _ = open_window(app, true);
        }
        NowChoice::Install => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { install(&app, Mode::Now).await });
        }
        NowChoice::Show => {
            let _ = open_window(app, true);
            if matches!(snapshot.phase, Phase::Idle | Phase::UpToDate) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { check(&app, true).await });
            }
        }
    }
    choice
}

/// "After the call": the verified update waits until no Talk has been seen for
/// a few polls in a row, or until "Update now" ends the wait.
pub(super) async fn wait_for_call_end<R: Runtime>(
    app: &AppHandle<R>,
    update: &Update,
    bytes: Vec<u8>,
) {
    let state = app.state::<NativeUpdates>();
    state.with(|i| {
        i.snapshot.phase = Phase::Waiting;
        i.force_now = false;
    });
    let mut quiet = 0;
    loop {
        let forced = state.inner.lock().is_ok_and(|i| i.force_now);
        if call_over(quiet, forced) {
            break;
        }
        tokio::time::sleep(TALK_POLL).await;
        quiet = if state.in_talk() { 0 } else { quiet + 1 };
    }
    // The wait may have been long: the trust statement must still be fresh.
    if !installs_allowed(app) {
        return fail(&state, Failure::Untrusted);
    }
    if crate::web_trust::updater_key(&state.source.pubkey, &update.signature).is_err() {
        return fail(&state, Failure::Verify);
    }
    apply_now(app, update, &bytes);
}

/// Installs verified bytes and restarts. A failure leaves the installed
/// version as it was and says so.
pub(super) fn apply_now<R: Runtime>(app: &AppHandle<R>, update: &Update, bytes: &[u8]) {
    let state = app.state::<NativeUpdates>();
    state.with(|i| i.snapshot.phase = Phase::Installing);
    let done = if state.source.portable {
        replace_portable(&state, update, bytes)
    } else {
        // Windows: the installer runs and this process has exited. Elsewhere
        // the new version replaced the old one on disk. The plugin verifies
        // the bytes once more before it installs them.
        update.install(bytes).map_err(|_| Failure::Install)
    };
    match done {
        Ok(()) => restart(app, &state),
        Err(failure) => fail(&state, failure),
    }
}

fn replace_portable(state: &NativeUpdates, update: &Update, bytes: &[u8]) -> Result<(), Failure> {
    // Same check as the installer path: never act on bytes that do not verify.
    update.verify(bytes).map_err(|_| Failure::Verify)?;
    let exe = match &state.source.executable_path {
        Some(path) => path.clone(),
        None => std::env::current_exe().map_err(|_| Failure::Install)?,
    };
    replace(bytes, &exe).map_err(|e| match e {
        PortableError::Package => Failure::Verify,
        PortableError::Folder => Failure::Folder,
        PortableError::Swap => Failure::Install,
    })
}

fn restart<R: Runtime>(app: &AppHandle<R>, state: &NativeUpdates) {
    let hook = state.restart.lock().ok().and_then(|h| h.clone());
    match hook {
        Some(hook) => hook(),
        None => app.restart(),
    }
}
