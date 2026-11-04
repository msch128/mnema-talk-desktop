//! Downloading and installing a verified update, and applying the installer that
//! was kept for the next start.
use super::START_TIMEOUT;
use super::check::{check, classify, fetch};
use super::logic::{disabled, offerable, over_cap, verify_package};
use super::state::{Failure, Mode, NativeUpdates, Pending, Phase, Source};
use super::window::open_window;
use crate::web_bundle::write_atomic;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::Update;

/// Whether a fresh trust statement allows installing anything new. A host
/// without the web client state (tests, the plain preview) is not restricted.
pub(super) fn installs_allowed<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<crate::web_client::Trusted>()
        .is_none_or(|t| t.fresh())
}

/// Fetches the trust statement now, within the start timeout. Only for the
/// pinned production endpoints: tests run against stand-in servers and must
/// not reach the real statement.
async fn refresh_trust<R: Runtime>(app: &AppHandle<R>, source: &Source) {
    if !source.github_only {
        return;
    }
    if let Some(trusted) = app.try_state::<crate::web_client::Trusted>() {
        let _ = tokio::time::timeout(START_TIMEOUT, trusted.trust_round()).await;
    }
}

/// Download, verify and either install at once (`Mode::Now`) or keep the
/// verified installer for the next start (`Mode::Later`).
pub async fn install<R: Runtime>(app: &AppHandle<R>, mode: Mode) {
    let state = app.state::<NativeUpdates>();
    if state.source.portable {
        return;
    }
    // After a restart a held installer has no manifest entry in memory yet.
    if state
        .inner
        .lock()
        .is_ok_and(|i| i.update.is_none() && !i.busy)
    {
        check(app, false).await;
    }
    let Some(update) = start_download(&state, mode) else {
        return;
    };
    // The statement may have changed since the check. Without a fresh one
    // (expired, or its fetch failed this round) nothing new is installed. A
    // client that started offline fetches it now instead of waiting for the
    // next round, and says the trust is missing rather than blaming the
    // signature.
    if !installs_allowed(app) {
        refresh_trust(app, &state.source).await;
    }
    if !installs_allowed(app) {
        return fail(&state, Failure::Untrusted);
    }
    if crate::web_trust::updater_key(&state.source.pubkey, &update.signature).is_err() {
        return fail(&state, Failure::Verify);
    }
    let bytes = match fetch_installer(app, &state, &update).await {
        Ok(bytes) => bytes,
        Err(failure) => return fail(&state, failure),
    };
    match mode {
        Mode::Later => keep_for_next_start(&state, &update, &bytes),
        Mode::Now => {
            state.with(|i| i.snapshot.phase = Phase::Installing);
            match update.install(&bytes) {
                // Windows: the installer runs and this process has exited.
                // Elsewhere the new version replaced the old one on disk.
                Ok(()) => app.restart(),
                Err(_) => fail(&state, Failure::Install),
            }
        }
    }
}

/// Claims the updater for a download of the offered update, or `None` when it
/// is busy or nothing is offered.
fn start_download(state: &NativeUpdates, mode: Mode) -> Option<Update> {
    state
        .with(|i| {
            if i.busy || i.update.is_none() {
                return None;
            }
            i.busy = true;
            i.snapshot.phase = Phase::Downloading;
            i.snapshot.mode = Some(mode);
            i.snapshot.received = 0;
            i.snapshot.total = None;
            i.snapshot.error = None;
            i.update.clone()
        })
        .flatten()
}

/// The installer bytes: the verified one kept on disk, else a fresh download
/// that is abandoned as soon as it grows beyond the installer size cap (`over_cap`).
async fn fetch_installer<R: Runtime>(
    app: &AppHandle<R>,
    state: &NativeUpdates,
    update: &Update,
) -> Result<Vec<u8>, Failure> {
    if let Some(bytes) = held_installer(state, update) {
        return Ok(bytes);
    }
    let progress = app.clone();
    let too_big = Arc::new(tokio::sync::Notify::new());
    let signal = too_big.clone();
    let download = update.download(
        move |chunk, total| {
            // The phase does not change while bytes arrive, so the menu
            // is left alone.
            let state = progress.state::<NativeUpdates>();
            if let Ok(mut i) = state.inner.lock() {
                i.snapshot.received += chunk as u64;
                i.snapshot.total = total;
                if over_cap(i.snapshot.received, i.snapshot.total) {
                    signal.notify_one();
                }
            }
        },
        || {},
    );
    // Dropping the download stops it.
    tokio::select! {
        result = download => result.map_err(|e| classify(&e)),
        () = too_big.notified() => Err(Failure::Verify),
    }
}

/// `Mode::Later`: the verified installer waits on disk for the next start.
fn keep_for_next_start(state: &NativeUpdates, update: &Update, bytes: &[u8]) {
    let stored = state
        .pending_path()
        .is_some_and(|p| write_atomic(&p, bytes).is_ok());
    if !stored {
        return fail(state, Failure::Install);
    }
    state.with(|i| {
        i.busy = false;
        i.persisted.pending = Some(Pending {
            version: update.version.clone(),
            signature: update.signature.clone(),
        });
        i.persisted.save(state.dir.as_deref());
        i.snapshot.phase = Phase::Ready;
    });
}

/// The installer kept for the next start, if it is this very update and still
/// verifies; saves downloading it a second time.
pub(super) fn held_installer(state: &NativeUpdates, update: &Update) -> Option<Vec<u8>> {
    let held = state.inner.lock().ok()?.persisted.pending.clone()?;
    if held.version != update.version {
        return None;
    }
    let bytes = std::fs::read(state.pending_path()?).ok()?;
    let key = crate::web_trust::updater_key(&state.source.pubkey, &update.signature).ok()?;
    verify_package(&bytes, &update.signature, &key, &update.version)
        .is_ok()
        .then_some(bytes)
}

pub(super) fn fail(state: &NativeUpdates, failure: Failure) {
    state.with(|i| {
        i.busy = false;
        i.snapshot.phase = Phase::Failed;
        i.snapshot.error = Some(failure);
    });
}

pub fn skip<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<NativeUpdates>();
    state.with(|i| {
        if i.busy {
            return;
        }
        if let Some(version) = i.snapshot.version.clone() {
            i.persisted.skipped = Some(version);
            i.persisted.save(state.dir.as_deref());
        }
        i.update = None;
        i.snapshot.phase = Phase::Idle;
        i.snapshot.version = None;
        i.snapshot.notes = None;
        i.snapshot.error = None;
    });
}

/// An installer kept for this start, checked against a freshly fetched
/// manifest: it must be the newest release and still verify. Bounded in time
/// so that start is never held up by the network.
pub async fn prepare_pending<R: Runtime>(app: &AppHandle<R>) -> Option<(Update, Vec<u8>)> {
    // Kept for later when no fresh trust statement is in force.
    if !installs_allowed(app) {
        return None;
    }
    let state = app.state::<NativeUpdates>();
    if state.source.portable {
        return None;
    }
    let (pending, current) = match state.inner.lock() {
        Ok(i) => (i.persisted.pending.clone()?, i.snapshot.current.clone()),
        Err(_) => return None,
    };
    let source = state.source.clone();
    let found = tokio::time::timeout(START_TIMEOUT, fetch(app, &source)).await;
    let update = match found {
        Ok(Ok(Some(u))) if u.version == pending.version => u,
        // Newer release or none: the held installer is stale.
        Ok(Ok(_)) => {
            discard_pending(&state);
            return None;
        }
        // Offline: keep it for the next start.
        _ => return None,
    };
    let bytes = state.pending_path().and_then(|p| std::fs::read(p).ok());
    let key = crate::web_trust::updater_key(&source.pubkey, &update.signature).ok();
    let verified = bytes.as_deref().is_some_and(|b| {
        offerable(&current, &update.version).is_some()
            && key
                .as_deref()
                .is_some_and(|k| verify_package(b, &update.signature, k, &update.version).is_ok())
    });
    if !verified {
        discard_pending(&state);
        return None;
    }
    Some((update, bytes?))
}

pub(super) async fn apply_pending<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<NativeUpdates>();
    if disabled() || state.source.portable {
        return;
    }
    // The installer is only run on the strength of a trust statement fetched
    // in this process: fetch it first. That runs off the start path, so the
    // windows may already be open; the busy flag below keeps a check or an
    // "Update now" from overlapping the apply. If the fetch fails the installer
    // stays kept and the next start tries again.
    if state
        .inner
        .lock()
        .is_ok_and(|i| i.persisted.pending.is_some())
        && !installs_allowed(app)
    {
        refresh_trust(app, &state.source).await;
    }
    let Some((update, bytes)) = prepare_pending(app).await else {
        return;
    };
    let claimed = state.with(|i| {
        if i.busy {
            return false;
        }
        i.busy = true;
        i.snapshot.phase = Phase::Installing;
        i.snapshot.version = Some(update.version.clone());
        true
    });
    // Another run owns the updater right now: the installer stays kept.
    if claimed != Some(true) {
        return;
    }
    let _ = open_window(app, false);
    // On macOS this may ask for the administrator password.
    let installed = tauri::async_runtime::spawn_blocking(move || update.install(&bytes)).await;
    match installed {
        Ok(Ok(())) => app.restart(),
        _ => {
            discard_pending(&state);
            state.with(|i| i.busy = false);
            fail(&state, Failure::Install);
        }
    }
}

pub(super) fn discard_pending(state: &NativeUpdates) {
    state.with(|i| {
        i.persisted.pending = None;
        i.persisted.save(state.dir.as_deref());
        if i.snapshot.phase == Phase::Ready {
            i.snapshot.phase = Phase::Idle;
            i.snapshot.version = None;
        }
    });
    if let Some(p) = state.pending_path() {
        let _ = std::fs::remove_file(p);
    }
}
