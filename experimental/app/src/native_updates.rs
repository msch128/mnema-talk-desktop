//! Signed self-update of the desktop client itself, through the official Tauri
//! updater plugin. The web app has its own signed bundles
//! ([`crate::web_updates`]); this replaces the installed program.
//!
//! Where bytes come from decides nothing. The release manifest names an
//! installer and a minisign signature; the installer is run only after the
//! signature verifies against the public key pinned in `tauri.conf.json`, and
//! the signed version must equal the announced one (`requireSignedVersion`),
//! so a replayed older installer cannot be paired with a newer version number.
//! The plugin only installs versions greater than the running one.
//!
//! "On next start" keeps the verified installer on disk. It is verified again
//! against a freshly fetched manifest before it runs.
use crate::web_bundle::{CHECK_INTERVAL_SECS, Version, write_atomic};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

/// Poll granularity of the scheduler; a check runs when the six-hour interval
/// shared with the web bundles has passed.
const POLL: Duration = Duration::from_secs(15 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(8);
const NOTES_MAX: usize = 2000;
/// The plugin buffers the whole installer in memory before its signature can
/// be checked, so a download beyond this is abandoned.
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;
const STATE_FILE: &str = "native-update.json";
const PENDING_FILE: &str = "native-update.pending";
const WINDOW: &str = "updater";
pub const MENU_ID: &str = "native-update";
const UI_PREFIX: &str = "/__update/";

/// Shipped only inside the portable ZIP, next to the executable, by the release
/// workflow. The installer never carries it, so an installed copy keeps
/// updating itself; planting the file can only switch updating off.
pub const PORTABLE_MARKER: &str = "mnema-portable.marker";

/// Where a release is announced to a portable copy, which must not run the
/// installer: a second copy would be installed beside it.
const RELEASE_PAGE: &str = "https://github.com/msch128/mnema-talk-desktop/releases/tag/v";

pub fn disabled() -> bool {
    crate::web_updates::disabled()
}

/// Whether the executable at `exe` is the portable build: the marker is a
/// regular file in the same directory (a link or a directory does not count).
pub fn portable_at(exe: &Path) -> bool {
    exe.parent()
        .and_then(|dir| std::fs::symlink_metadata(dir.join(PORTABLE_MARKER)).ok())
        .is_some_and(|m| m.file_type().is_file())
}

/// Whether the running program is the portable build. An unknown location is
/// not portable: only the marker switches the installer off.
pub fn is_portable() -> bool {
    std::env::current_exe().is_ok_and(|exe| portable_at(&exe))
}

/// The release page of a version, only for a plain `MAJOR.MINOR.PATCH`.
pub fn release_page(version: &str) -> Option<Url> {
    let version = Version::parse(version)?;
    Url::parse(&format!("{RELEASE_PAGE}{version}")).ok()
}

// ---------------------------------------------------------------- pure logic

/// Whether the interval has passed. A clock set back counts as due.
pub fn check_due(last_check: u64, now: u64) -> bool {
    last_check > now || now - last_check >= CHECK_INTERVAL_SECS
}

/// The candidate version when it is strictly newer than `current`. Native
/// versions are plain `MAJOR.MINOR.PATCH`; anything else is not offered.
pub fn offerable(current: &str, candidate: &str) -> Option<Version> {
    let candidate = Version::parse(candidate)?;
    (candidate > Version::parse(current)?).then_some(candidate)
}

/// A skipped version stays quiet; a newer one is offered again.
pub fn is_skipped(skipped: Option<&str>, candidate: &str) -> bool {
    skipped == Some(candidate)
}

/// The installer is fetched from GitHub's release downloads over HTTPS (which
/// redirects to GitHub's asset hosts). The signature decides trust; this keeps
/// the request from going elsewhere in the first place.
pub fn download_url_ok(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Key,
    Signature,
    Mismatch,
    Version,
}

/// Minisign verification of a package, the same rule the plugin applies to a
/// fresh download, for the installer that waited on disk.
pub fn verify_package(
    bytes: &[u8],
    signature: &str,
    pubkey: &str,
    version: &str,
) -> Result<(), VerifyError> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = |s: &str| {
        b64.decode(s.trim())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let key = text(pubkey)
        .and_then(|t| minisign_verify::PublicKey::decode(&t).ok())
        .ok_or(VerifyError::Key)?;
    let sig = text(signature)
        .and_then(|t| minisign_verify::Signature::decode(&t).ok())
        .ok_or(VerifyError::Signature)?;
    key.verify(bytes, &sig, true)
        .map_err(|_| VerifyError::Mismatch)?;
    // Only after `verify` is the trusted comment covered by the signature.
    let signed = sig
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or(VerifyError::Version)?;
    let signed = Version::parse(signed.trim_start_matches('v')).ok_or(VerifyError::Version)?;
    (Version::parse(version) == Some(signed))
        .then_some(())
        .ok_or(VerifyError::Version)
}

// ----------------------------------------------------------------- state

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Installing,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Now,
    Later,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Offline,
    Verify,
    Install,
    Unknown,
}

/// What the update window shows. No raw error text crosses to the page.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Snapshot {
    pub current: String,
    pub phase: Phase,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub received: u64,
    pub total: Option<u64>,
    pub mode: Option<Mode>,
    pub error: Option<Failure>,
    /// The portable build never runs an installer: it only points to the release.
    pub portable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Pending {
    version: String,
    signature: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Persisted {
    last_check: u64,
    skipped: Option<String>,
    pending: Option<Pending>,
}

impl Persisted {
    fn load(dir: Option<&Path>) -> Persisted {
        dir.and_then(|d| std::fs::read(d.join(STATE_FILE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, dir: Option<&Path>) {
        let Some(dir) = dir else { return };
        let _ = write_atomic(
            &dir.join(STATE_FILE),
            &serde_json::to_vec(self).unwrap_or_default(),
        );
    }
}

struct Inner {
    persisted: Persisted,
    snapshot: Snapshot,
    update: Option<Update>,
    busy: bool,
}

/// The pinned public key and endpoints, normally read from `tauri.conf.json`.
#[derive(Clone)]
pub struct Source {
    pub endpoints: Vec<Url>,
    pub pubkey: String,
    /// Production only downloads from github.com; tests use a loopback server.
    pub github_only: bool,
    pub target: Option<String>,
    pub executable_path: Option<PathBuf>,
    pub portable: bool,
}

impl Source {
    pub fn pinned<R: Runtime>(app: &AppHandle<R>) -> Result<Source, String> {
        let config = app
            .config()
            .plugins
            .0
            .get("updater")
            .ok_or("The updater is not configured.")?;
        let pubkey = config
            .get("pubkey")
            .and_then(|v| v.as_str())
            .filter(|k| !k.is_empty())
            .ok_or("The updater has no public key.")?
            .to_owned();
        let endpoints = config
            .get("endpoints")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().and_then(|s| Url::parse(s).ok()))
            .collect::<Vec<_>>();
        if endpoints.is_empty() || endpoints.iter().any(|e| e.scheme() != "https") {
            return Err("The updater needs HTTPS endpoints.".into());
        }
        Ok(Source {
            endpoints,
            pubkey,
            github_only: true,
            target: None,
            executable_path: None,
            portable: is_portable(),
        })
    }
}

/// Sets the text of one menu entry (every window has its own menu).
type MenuText = Arc<dyn Fn(&str) + Send + Sync>;

/// The menu entries and the text they show now. Setting a text is a round
/// trip to the main thread, so it is only done when the text changes and never
/// with the lock held.
#[derive(Default)]
struct Menu {
    items: Vec<MenuText>,
    shown: Option<&'static str>,
}

pub struct NativeUpdates {
    inner: Mutex<Inner>,
    dir: Option<PathBuf>,
    source: Source,
    menu: Mutex<Menu>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl NativeUpdates {
    pub fn new(current: String, dir: Option<PathBuf>, source: Source) -> NativeUpdates {
        let mut persisted = Persisted::load(dir.as_deref());
        let portable = source.portable;
        if portable {
            // A portable copy never installs: forget what an earlier run kept.
            persisted.pending = None;
            if let Some(d) = &dir {
                let _ = std::fs::remove_file(d.join(PENDING_FILE));
            }
        }
        // Already installed or superseded: nothing to wait for.
        if let Some(p) = &persisted.pending
            && offerable(&current, &p.version).is_none()
        {
            persisted.pending = None;
            if let Some(d) = &dir {
                let _ = std::fs::remove_file(d.join(PENDING_FILE));
            }
            persisted.save(dir.as_deref());
        }
        let (phase, version) = match &persisted.pending {
            Some(p) => (Phase::Ready, Some(p.version.clone())),
            None => (Phase::Idle, None),
        };
        NativeUpdates {
            inner: Mutex::new(Inner {
                persisted,
                snapshot: Snapshot {
                    current,
                    phase,
                    version,
                    notes: None,
                    received: 0,
                    total: None,
                    mode: None,
                    error: None,
                    portable,
                },
                update: None,
                busy: false,
            }),
            dir,
            source,
            menu: Mutex::new(Menu::default()),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner
            .lock()
            .map(|i| i.snapshot.clone())
            .unwrap_or_else(|_| Snapshot {
                current: String::new(),
                phase: Phase::Idle,
                version: None,
                notes: None,
                received: 0,
                total: None,
                mode: None,
                error: Some(Failure::Unknown),
                portable: false,
            })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> Option<T> {
        let mut inner = self.inner.lock().ok()?;
        let out = f(&mut inner);
        drop(inner);
        self.refresh_menu();
        Some(out)
    }

    fn refresh_menu(&self) {
        let snapshot = self.snapshot();
        let text = menu_text(snapshot.phase, snapshot.portable);
        let items = match self.menu.lock() {
            Ok(mut menu) if menu.shown != Some(text) => {
                menu.shown = Some(text);
                menu.items.clone()
            }
            _ => return,
        };
        for set in items {
            set(text);
        }
    }

    fn pending_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(PENDING_FILE))
    }
}

/// A dot marks a waiting update; the word is the same in every language. A
/// portable copy says it only has a download to offer.
fn menu_text(phase: Phase, portable: bool) -> &'static str {
    match phase {
        Phase::Available if portable => "Download update \u{2022}",
        Phase::Available | Phase::Ready | Phase::Failed => "Update \u{2022}",
        _ => "Updates",
    }
}

pub fn menu_item<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::MenuItem<R>> {
    let state = app.state::<NativeUpdates>();
    let item = tauri::menu::MenuItem::with_id(
        app,
        MENU_ID,
        menu_text(state.snapshot().phase, state.snapshot().portable),
        true,
        None::<&str>,
    )?;
    let handle = item.clone();
    if let Ok(mut menu) = state.menu.lock() {
        menu.items.push(Arc::new(move |text| {
            let _ = handle.set_text(text);
        }));
    }
    Ok(item)
}

// ------------------------------------------------------------- operations

fn classify(error: &tauri_plugin_updater::Error) -> Failure {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => Failure::Verify,
        E::Reqwest(_) | E::Network(_) | E::ReleaseNotFound | E::Io(_) => Failure::Offline,
        _ => Failure::Unknown,
    }
}

/// Whether an installer download is already beyond what is accepted, by its
/// announced length or by what has arrived.
pub fn over_cap(received: u64, total: Option<u64>) -> bool {
    received > MAX_INSTALLER || total.is_some_and(|t| t > MAX_INSTALLER)
}

/// Whether a fresh trust statement allows installing anything new. A host
/// without the web client state (tests, the plain preview) is not restricted.
fn installs_allowed<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<crate::web_client::Trusted>()
        .is_none_or(|t| t.fresh())
}

fn updater<R: Runtime>(
    app: &AppHandle<R>,
    source: &Source,
    pubkey: &str,
    timeout: Duration,
) -> Result<tauri_plugin_updater::Updater, tauri_plugin_updater::Error> {
    let mut builder = app
        .updater_builder()
        .pubkey(pubkey.to_owned())
        .timeout(timeout);
    builder = builder.endpoints(source.endpoints.clone())?;
    if let Some(target) = &source.target {
        builder = builder.target(target.clone());
    }
    if let Some(path) = &source.executable_path {
        builder = builder.executable_path(path);
    }
    builder.build()
}

async fn fetch<R: Runtime>(app: &AppHandle<R>, source: &Source) -> Result<Option<Update>, Failure> {
    let found = check_with(app, source, &source.pubkey).await?;
    // The trust statement decides which updater key an update may be signed
    // with: a revoked key is refused, a listed replacement key is used.
    let Some(update) = found else { return Ok(None) };
    let key = crate::web_trust::updater_key(&source.pubkey, &update.signature)
        .map_err(|_| Failure::Verify)?;
    let update = if key == source.pubkey {
        update
    } else {
        let again = check_with(app, source, &key)
            .await?
            .ok_or(Failure::Verify)?;
        if again.version != update.version || again.signature != update.signature {
            return Err(Failure::Verify);
        }
        again
    };
    if source.github_only && !download_url_ok(&update.download_url) {
        // Not "nothing newer": that would also discard a kept installer.
        return Err(Failure::Verify);
    }
    Ok(Some(update))
}

async fn check_with<R: Runtime>(
    app: &AppHandle<R>,
    source: &Source,
    pubkey: &str,
) -> Result<Option<Update>, Failure> {
    let updater = updater(app, source, pubkey, DOWNLOAD_TIMEOUT).map_err(|e| classify(&e))?;
    tokio::time::timeout(CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| Failure::Offline)?
        .map_err(|e| classify(&e))
}

fn trim_notes(body: Option<String>) -> Option<String> {
    body.map(|b| {
        b.chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(NOTES_MAX)
            .collect()
    })
    .filter(|b: &String| !b.trim().is_empty())
}

/// Look for a newer version. `manual` shows the outcome (also failures);
/// a background check stays silent unless it finds something new.
pub async fn check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    if look(app, manual).await {
        let _ = open_window(app, false);
    }
}

/// The check itself; true when a new, not skipped version should be announced.
async fn look<R: Runtime>(app: &AppHandle<R>, manual: bool) -> bool {
    let state = app.state::<NativeUpdates>();
    let started = state.with(|i| {
        if i.busy {
            return false;
        }
        i.busy = true;
        if manual {
            i.snapshot.phase = Phase::Checking;
            i.snapshot.error = None;
        }
        true
    });
    if started != Some(true) {
        return false;
    }
    let found = fetch(app, &state.source).await;
    let mut announce = false;
    state.with(|i| {
        i.busy = false;
        let current = i.snapshot.current.clone();
        // A failed check is not a check: the next poll tries again.
        if found.is_ok() {
            i.persisted.last_check = now();
        }
        match found {
            Ok(Some(update)) if offerable(&current, &update.version).is_some() => {
                let version = update.version.clone();
                let ready = i
                    .persisted
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.version == version);
                i.snapshot.version = Some(version.clone());
                i.snapshot.notes = trim_notes(update.body.clone());
                i.snapshot.received = 0;
                i.snapshot.total = None;
                i.snapshot.mode = None;
                i.snapshot.error = None;
                i.snapshot.phase = if ready {
                    Phase::Ready
                } else {
                    Phase::Available
                };
                announce = !ready && !is_skipped(i.persisted.skipped.as_deref(), &version);
                i.update = Some(update);
            }
            Ok(_) => {
                // Nothing newer. A held installer for a version that is no
                // longer offered is stale.
                i.update = None;
                if i.persisted.pending.take().is_some()
                    && let Some(d) = &state.dir
                {
                    let _ = std::fs::remove_file(d.join(PENDING_FILE));
                }
                i.snapshot.version = None;
                i.snapshot.notes = None;
                i.snapshot.phase = if manual { Phase::UpToDate } else { Phase::Idle };
            }
            Err(failure) => {
                if manual {
                    i.snapshot.phase = Phase::Failed;
                    i.snapshot.error = Some(failure);
                } else if matches!(i.snapshot.phase, Phase::Checking) {
                    i.snapshot.phase = Phase::Idle;
                }
            }
        }
        i.persisted.save(state.dir.as_deref());
    });
    announce
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
    let taken = state.with(|i| {
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
    });
    let Some(Some(update)) = taken else { return };
    // The statement may have changed since the check. Without a fresh one
    // (expired, or its fetch failed this round) nothing new is installed.
    if !installs_allowed(app)
        || crate::web_trust::updater_key(&state.source.pubkey, &update.signature).is_err()
    {
        return fail(&state, Failure::Verify);
    }
    let progress = app.clone();
    let held = held_installer(&state, &update);
    let downloaded = if let Some(bytes) = held {
        Ok(bytes)
    } else {
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
    };
    let bytes = match downloaded {
        Ok(bytes) => bytes,
        Err(failure) => return fail(&state, failure),
    };
    match mode {
        Mode::Later => {
            let stored = state
                .pending_path()
                .is_some_and(|p| write_atomic(&p, &bytes).is_ok());
            if !stored {
                return fail(&state, Failure::Install);
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

/// The installer kept for the next start, if it is this very update and still
/// verifies; saves downloading it a second time.
fn held_installer(state: &NativeUpdates, update: &Update) -> Option<Vec<u8>> {
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

fn fail(state: &NativeUpdates, failure: Failure) {
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

async fn apply_pending<R: Runtime>(app: &AppHandle<R>) {
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
        && let Some(trusted) = app.try_state::<crate::web_client::Trusted>()
    {
        let _ = tokio::time::timeout(START_TIMEOUT, trusted.trust_round()).await;
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

fn discard_pending(state: &NativeUpdates) {
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

// ------------------------------------------------------------ app wiring

/// Registers the state, applies an installer kept for this start and starts
/// the scheduler. Call once from `setup`.
pub fn setup<R: Runtime>(app: &AppHandle<R>) {
    let Ok(source) = Source::pinned(app) else {
        eprintln!("Mnema Desktop: native updates are not configured.");
        return;
    };
    let dir = crate::storage::data_dir(app);
    let current = app.package_info().version.to_string();
    app.manage(NativeUpdates::new(current, dir, source));
    if disabled() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Off the main thread: the trust and manifest fetches may take a while.
        apply_pending(&app).await;
        // A check at every start, then whenever the interval has passed.
        check(&app, false).await;
        loop {
            tokio::time::sleep(POLL).await;
            let due = {
                let state = app.state::<NativeUpdates>();
                state
                    .inner
                    .lock()
                    .is_ok_and(|i| check_due(i.persisted.last_check, now()))
            };
            if due {
                check(&app, false).await;
            }
        }
    });
}

/// The window is the host's own page, served from the executable and not from
/// the web bundle, so it also works when no bundle is usable.
pub fn open_window<R: Runtime>(app: &AppHandle<R>, focus: bool) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        if focus {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return Ok(());
    }
    let url = ui_base().join("index.html").expect("constant path");
    crate::web_permissions::with_probe_args(
        app,
        WINDOW,
        crate::storage::window_on_default_storage(
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::CustomProtocol(url))
                .use_https_scheme(true)
                .title("Mnema Desktop DEV \u{2014} Update")
                .inner_size(440.0, 320.0)
                .resizable(false)
                .maximizable(false)
                .focused(focus)
                .on_navigation(ui_url)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_permission_request(|webview, kind| {
                    let url = webview.url().ok();
                    crate::web_permissions::decide(webview.label(), url.as_ref(), kind)
                }),
        ),
    )
    .build()?;
    Ok(())
}

/// Menu entry: show the window and look again.
pub fn menu_clicked<R: Runtime>(app: &AppHandle<R>) {
    let _ = open_window(app, true);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let busy_or_waiting = {
            let state = app.state::<NativeUpdates>();
            matches!(
                state.snapshot().phase,
                Phase::Available | Phase::Ready | Phase::Downloading | Phase::Installing
            )
        };
        if !busy_or_waiting {
            check(&app, true).await;
        }
    });
}

fn ui_base() -> Url {
    crate::web_client::app_url()
        .join(UI_PREFIX.trim_start_matches('/'))
        .expect("constant path")
}

/// Only the update page itself may be shown in the update window.
pub fn ui_url(url: &Url) -> bool {
    let base = ui_base();
    url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port() == base.port()
        && url.path().starts_with(UI_PREFIX)
}

const INDEX: &str = include_str!("../update-ui/index.html");
const STYLE: &str = include_str!("../update-ui/update.css");
const SCRIPT: &str = include_str!("../update-ui/update.js");

/// Body and content type of an update page file, for the `updater` window.
pub fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path.strip_prefix(UI_PREFIX)? {
        "" | "index.html" => Some(("text/html; charset=utf-8", INDEX)),
        "update.css" => Some(("text/css; charset=utf-8", STYLE)),
        "update.js" => Some(("text/javascript; charset=utf-8", SCRIPT)),
        _ => None,
    }
}

pub const UI_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

fn from_update_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == WINDOW && window.url().is_ok_and(|u| ui_url(&u)) {
        Ok(())
    } else {
        Err("Only the update window may use this.".into())
    }
}

#[tauri::command]
pub fn native_update_state(
    window: WebviewWindow,
    state: tauri::State<'_, NativeUpdates>,
) -> Result<Snapshot, String> {
    from_update_window(&window)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub fn native_update_action(
    window: WebviewWindow,
    state: tauri::State<'_, NativeUpdates>,
    action: String,
) -> Result<Snapshot, String> {
    from_update_window(&window)?;
    let app = window.app_handle().clone();
    let portable = state.source.portable;
    match action.as_str() {
        // The portable copy cannot be replaced by an installer.
        "now" | "later" if portable => {
            return Err("A portable copy does not install updates.".into());
        }
        "download_page" => {
            let page = portable
                .then(|| state.snapshot().version)
                .flatten()
                .and_then(|v| release_page(&v));
            if let Some(url) = page {
                // The confirmation dialog waits for an answer.
                std::thread::spawn(move || crate::web_client::open_external(&url));
            }
        }
        // The link of the footer: the page of the offered version, else of the
        // installed one. The address is built here from the fixed prefix.
        "release_page" => {
            let snapshot = state.snapshot();
            let version = snapshot.version.unwrap_or(snapshot.current);
            if let Some(url) = release_page(&version) {
                std::thread::spawn(move || crate::web_client::open_external(&url));
            }
        }
        "check" => {
            tauri::async_runtime::spawn(async move { check(&app, true).await });
        }
        "now" => {
            tauri::async_runtime::spawn(async move { install(&app, Mode::Now).await });
        }
        "later" => {
            tauri::async_runtime::spawn(async move { install(&app, Mode::Later).await });
        }
        "skip" => skip(&app),
        "close" => {
            window.close().map_err(|_| "Cannot close the window.")?;
        }
        _ => return Err("Unknown action.".into()),
    }
    Ok(state.snapshot())
}

#[cfg(test)]
mod tests;
