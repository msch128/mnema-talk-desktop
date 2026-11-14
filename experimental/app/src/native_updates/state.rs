//! The state of the self-update: what is persisted and what the update window,
//! the rail and the menu show.
use super::logic::{PORTABLE_TARGET, is_portable, offerable};
use super::menu::Menu;
use super::{PENDING_FILE, STATE_FILE};
use crate::web_bundle::write_atomic;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::Update;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    /// Downloaded and verified; installs as soon as the Talk has ended.
    Waiting,
    Installing,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Now,
    Later,
    /// Download and verify now, install when the Talk has ended.
    AfterCall,
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
    /// The portable copy cannot write to its own folder, so it cannot replace itself.
    Folder,
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
    /// The portable build replaces its own files instead of running an installer.
    pub portable: bool,
    /// "Update now" was asked for during a Talk: the page asks whether to wait for its end.
    pub confirm_call: bool,
    /// The person skipped this version: the rail shows no pill for it.
    pub skipped: bool,
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
    /// "Update now" while waiting for a Talk to end: install without waiting.
    pub(super) force_now: bool,
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
        // Only the update self-test (a build feature no release enables) may
        // read a feed on this machine, which its CI job serves.
        let loopback = cfg!(feature = "update-selftest")
            && !endpoints.is_empty()
            && endpoints
                .iter()
                .all(|e| e.scheme() == "http" && e.host_str() == Some("127.0.0.1"));
        if endpoints.is_empty() || !(loopback || endpoints.iter().all(|e| e.scheme() == "https")) {
            return Err("The updater needs HTTPS endpoints.".into());
        }
        // The portable copy reads its own entry of the feed, the signed ZIP.
        let portable = cfg!(windows) && is_portable();
        Ok(Source {
            endpoints,
            pubkey,
            github_only: !loopback,
            target: portable.then(|| PORTABLE_TARGET.to_owned()),
            executable_path: None,
            portable,
        })
    }
}

/// Whether the person is in a Talk right now.
pub(super) type TalkProbe = Arc<dyn Fn() -> bool + Send + Sync>;
/// Shows a new state where it is drawn outside the update window (the rail).
pub(super) type Push = Arc<dyn Fn(&Snapshot) + Send + Sync>;

pub struct NativeUpdates {
    pub(super) inner: Mutex<Inner>,
    pub(super) dir: Option<PathBuf>,
    pub(super) source: Source,
    pub(super) menu: Mutex<Menu>,
    pub(super) talk: Mutex<TalkProbe>,
    pub(super) push: Mutex<Option<Push>>,
    /// Replaces the restart after an install (the update self-test records first; tests stay alive).
    pub(super) restart: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
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
                    confirm_call: false,
                    skipped: false,
                },
                update: None,
                busy: false,
                force_now: false,
            }),
            dir,
            source,
            menu: Mutex::new(Menu::default()),
            talk: Mutex::new(Arc::new(|| false)),
            push: Mutex::new(None),
            restart: Mutex::new(None),
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
                confirm_call: false,
                skipped: false,
            })
    }

    pub(super) fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> Option<T> {
        let mut inner = self.inner.lock().ok()?;
        let out = f(&mut inner);
        drop(inner);
        self.changed();
        Some(out)
    }

    /// The state changed: the menu and the rail follow.
    pub(super) fn changed(&self) {
        self.refresh_menu();
        let push = self.push.lock().ok().and_then(|p| p.clone());
        if let Some(push) = push {
            push(&self.snapshot());
        }
    }

    pub(super) fn in_talk(&self) -> bool {
        let probe = self.talk.lock().ok().map(|t| t.clone());
        probe.is_some_and(|t| t())
    }

    /// Sets how the app learns whether a Talk is running (the server pages report it).
    pub fn set_talk_probe(&self, probe: impl Fn() -> bool + Send + Sync + 'static) {
        if let Ok(mut t) = self.talk.lock() {
            *t = Arc::new(probe);
        }
    }

    pub(super) fn pending_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(PENDING_FILE))
    }
}
