//! The state of the self-update: what is persisted, what the update window
//! shows and the menu entry that announces a waiting update.
use super::logic::{is_portable, offerable};
use super::{PENDING_FILE, STATE_FILE};
use crate::web_bundle::write_atomic;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::Update;
use url::Url;

pub const MENU_ID: &str = "native-update";

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
    /// No current trust statement could be fetched, so nothing new is
    /// installed; the signature is not at fault.
    Untrusted,
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
pub(super) struct Pending {
    pub(super) version: String,
    pub(super) signature: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Persisted {
    pub(super) last_check: u64,
    pub(super) skipped: Option<String>,
    pub(super) pending: Option<Pending>,
}

impl Persisted {
    pub(super) fn load(dir: Option<&Path>) -> Persisted {
        dir.and_then(|d| std::fs::read(d.join(STATE_FILE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub(super) fn save(&self, dir: Option<&Path>) {
        let Some(dir) = dir else { return };
        let _ = write_atomic(
            &dir.join(STATE_FILE),
            &serde_json::to_vec(self).unwrap_or_default(),
        );
    }
}

pub(super) struct Inner {
    pub(super) persisted: Persisted,
    pub(super) snapshot: Snapshot,
    pub(super) update: Option<Update>,
    pub(super) busy: bool,
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
pub(super) type MenuText = Arc<dyn Fn(&str) + Send + Sync>;

/// The menu entries and the text they show now. Setting a text is a round
/// trip to the main thread, so it is only done when the text changes and never
/// with the lock held.
#[derive(Default)]
pub(super) struct Menu {
    pub(super) items: Vec<MenuText>,
    shown: Option<&'static str>,
}

pub struct NativeUpdates {
    pub(super) inner: Mutex<Inner>,
    pub(super) dir: Option<PathBuf>,
    pub(super) source: Source,
    pub(super) menu: Mutex<Menu>,
}

pub(super) fn now() -> u64 {
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

    pub(super) fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> Option<T> {
        let mut inner = self.inner.lock().ok()?;
        let out = f(&mut inner);
        drop(inner);
        self.refresh_menu();
        Some(out)
    }

    pub(super) fn refresh_menu(&self) {
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

    pub(super) fn pending_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(PENDING_FILE))
    }
}

/// A dot marks a waiting update; the word is the same in every language. A
/// portable copy says it only has a download to offer.
pub(super) fn menu_text(phase: Phase, portable: bool) -> &'static str {
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
