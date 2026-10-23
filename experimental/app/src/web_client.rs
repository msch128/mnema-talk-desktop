//! The trusted desktop client. The web app runs from a signed bundle that this
//! host verified, at the host's own origin; the servers are reached through
//! the host (`web_proxy`, `web_ws`), so they supply data and never code. Each
//! server has its own page and its own session (`instances`, `web_shell`).
//! See mnema-talk `doc/desktop-trusted-client.md`.
use crate::instances::{
    Phase, Registry, Session, Stored, id_of_attachment_label, id_of_instance_label,
};
use crate::web_bundle::{Held, Refusal, ServerInfo, Store, TrustedKey, Version, trusted_keys};
use crate::web_proxy::{Resp, forward, now, serve_bundle};
use crate::web_trust::{Accepted, TrustState, root_key};
use crate::web_ws::WsEvent;
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Manager, Webview};
use url::Url;

const MAX_DISCOVERY_BYTES: usize = 16 * 1024;
const INSTANCES_FILE: &str = "instances.json";

/// The signed bundle compiled into this executable (empty when the build
/// had none; the client then has to download one before it can start).
static BUILTIN_BUNDLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/builtin-bundle.zip"));

/// Where the app lives. WebView2 shows a custom protocol as
/// `https://<scheme>.localhost`, the other engines as `<scheme>://localhost`.
#[cfg(windows)]
const APP_URL: &str = "https://mnema.localhost/";
#[cfg(not(windows))]
const APP_URL: &str = "mnema://localhost/";

pub fn app_url() -> Url {
    Url::parse(APP_URL).expect("constant URL")
}

/// `scheme://host` of a URL on the app origin. (`url::Origin` is opaque for a
/// custom scheme and never compares equal, so origins are compared as text.)
pub fn local_origin(url: &Url) -> Option<String> {
    let app = app_url();
    (url.scheme() == app.scheme()
        && url.host_str() == app.host_str()
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()))
}

/// An app page: the app origin and one of the app's own routes.
pub(super) fn bundled(url: &Url) -> bool {
    local_origin(url).is_some() && super::local_route::app_path(url.path())
}

/// The state of the trusted client: verified bundles and the servers.
pub struct Trusted {
    store: Mutex<Store>,
    /// Bundle keys in force: the trust statement's, else the compiled-in ones.
    keys: Mutex<Vec<TrustedKey>>,
    trust: Mutex<TrustState>,
    /// A fresh trust statement is in force and this round's fetch succeeded.
    /// False at start: only a fetch in this process sets it, so a blocked
    /// network cannot keep installing on the strength of a stored statement.
    /// Without it nothing new is installed (freeze protection); the already
    /// verified bundles keep running.
    fresh: std::sync::atomic::AtomicBool,
    /// The servers, their sessions and their counters.
    pub(crate) registry: Mutex<Registry>,
    /// The client replaces a single-server one whose browser storage may hold
    /// the end-to-end state of its server. True until the first server is
    /// added; that server takes the old storage over if it was the one that
    /// had a session here.
    adoptable: std::sync::atomic::AtomicBool,
    data_dir: Option<std::path::PathBuf>,
}

/// What the first server added does with the storage of the old client.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OldStorage {
    /// Nothing from an old client (or already decided).
    None,
    /// The server is plainly the owner and keeps it.
    Adopt,
    /// It cannot be attributed to one server: it is wiped.
    Wipe,
}

/// The rule: the owner is the one server that had a session, and no other
/// server did.
pub(crate) fn decide_old_storage(own_session: bool, other_sessions: usize) -> OldStorage {
    if own_session && other_sessions == 0 {
        OldStorage::Adopt
    } else {
        OldStorage::Wipe
    }
}

/// Something an earlier client leaves in its data directory, and the list of
/// servers (written by this one) does not exist yet.
fn old_client_left_data(dir: &std::path::Path) -> bool {
    !dir.join(INSTANCES_FILE).exists()
        && ["sessions", "web-trust", "web-bundles", "EBWebView"]
            .iter()
            .any(|name| dir.join(name).exists())
}

impl Trusted {
    pub(crate) fn new(data_dir: Option<std::path::PathBuf>, builtin: Option<&[u8]>) -> Trusted {
        // Before anything of this run is written: what is there is the old client's.
        let adoptable = data_dir.as_deref().is_some_and(old_client_left_data);
        let trust = TrustState::load(
            data_dir.as_ref().map(|d| d.join("web-trust")).as_deref(),
            root_key(),
        );
        crate::web_trust::publish(trust.statement());
        let keys = trust.effective_keys(trusted_keys());
        let store = Store::load(
            data_dir.as_ref().map(|d| d.join("web-bundles")).as_deref(),
            builtin,
            &keys,
        );
        let registry = Registry::restore(&Stored::load(
            data_dir.as_ref().map(|d| d.join(INSTANCES_FILE)).as_deref(),
        ));
        Trusted {
            store: Mutex::new(store),
            keys: Mutex::new(keys),
            fresh: std::sync::atomic::AtomicBool::new(false),
            trust: Mutex::new(trust),
            registry: Mutex::new(registry),
            adoptable: std::sync::atomic::AtomicBool::new(adoptable),
            data_dir,
        }
    }

    /// What becomes of the storage the single-server client left, decided once
    /// when the first server is added. That client kept every server it ever
    /// showed in one shared storage, so the storage can only be handed to a
    /// server that is plainly its owner: the one whose session is the only one
    /// the old client stored. With none, or with another session stored as
    /// well, the storage cannot be attributed and is wiped, never inherited by
    /// a server that may not have owned it.
    pub(crate) fn old_storage_for(
        &self,
        origin: &Url,
        vault: &dyn crate::web_vault::Vault,
    ) -> OldStorage {
        if !self
            .adoptable
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return OldStorage::None;
        }
        let own = vault.load(&crate::instances::id_for(origin)).is_some()
            || self.jar_file(origin).is_some_and(|f| f.exists());
        decide_old_storage(own, self.other_stored_sessions(origin))
    }

    /// Sessions of other servers that the old client left as files.
    fn other_stored_sessions(&self, origin: &Url) -> usize {
        let own = self.jar_file(origin);
        let Some(dir) = own.as_ref().and_then(|f| f.parent()) else {
            return 0;
        };
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().filter(|e| Some(e.path()) != own).count())
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn keys(&self) -> Vec<TrustedKey> {
        self.keys.lock().map(|k| k.clone()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_fresh(&self, fresh: bool) {
        self.fresh.store(fresh, std::sync::atomic::Ordering::SeqCst);
    }

    pub(super) fn fresh(&self) -> bool {
        self.fresh.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Takes a fetched trust statement. When it changes the keys in force,
    /// held bundles are checked again and a running session on a withdrawn
    /// bundle is ended. Returns whether bundles were dropped.
    ///
    /// Lock order is store, then keys (as in `install_current`), and the keys
    /// are replaced while the store lock is held: an install either finishes
    /// before and is reconciled here, or starts after and sees the new keys.
    fn apply_trust(&self, doc: &[u8], sig: &[u8], now: u64) -> Result<bool, String> {
        let mut store = self.store.lock().map_err(|_| "busy")?;
        let (keys, caps) = {
            let mut trust = self.trust.lock().map_err(|_| "busy")?;
            if trust.accept(doc, sig, now)? == Accepted::Unchanged {
                return Ok(false);
            }
            let caps = trust.statement().map(|s| s.caps()).unwrap_or_default();
            (trust.effective_keys(trusted_keys()), caps)
        };
        *self.keys.lock().map_err(|_| "busy")? = keys.clone();
        let dropped = self.enforce(&mut store, &keys, &caps);
        drop(store);
        Ok(dropped)
    }

    /// Re-checks the held bundles against the keys and caps in force and ends
    /// every session whose bundle (key and version) is no longer held, whether
    /// a revoked key or a cap withdrew it: that server's sockets close and its
    /// tab says why. Returns whether any bundle was withdrawn, so the caller
    /// reloads the pages.
    fn enforce(
        &self,
        store: &mut Store,
        keys: &[TrustedKey],
        caps: &std::collections::BTreeMap<u32, Version>,
    ) -> bool {
        let before = store.held().len();
        store.reconcile(keys);
        store.apply_caps(caps);
        let dropped = store.held().len() < before;
        let Ok(mut registry) = self.registry.lock() else {
            return dropped;
        };
        for id in registry.ids() {
            let Some(session) = registry.session(&id) else {
                continue;
            };
            let held = store.held().iter().any(|h| {
                h.bundle.key_id == session.held.bundle.key_id
                    && h.bundle.version == session.held.bundle.version
            });
            if !held {
                session.sockets.close_all();
                if let Some(slot) = registry.slot_mut(&id) {
                    slot.phase = Phase::Refused(WITHDRAWN.into());
                }
            }
        }
        dropped
    }

    /// One trust round with the result of a fetch: applies the statement,
    /// publishes it to the native updater and decides whether installs are
    /// allowed. Returns whether bundles were dropped.
    fn apply_round(&self, fetched: Result<(Vec<u8>, Vec<u8>), String>, now: u64) -> bool {
        let mut dropped = false;
        let mut ok = false;
        // Offline or unreachable: the last good statement stays in force.
        if let Ok((doc, sig)) = fetched {
            match self.apply_trust(&doc, &sig, now) {
                Ok(changed) => {
                    dropped = changed;
                    ok = true;
                }
                Err(why) => eprintln!("Mnema Desktop: trust statement refused: {why}"),
            }
        }
        let fresh = ok && self.trust.lock().is_ok_and(|t| t.fresh(now));
        self.fresh.store(fresh, std::sync::atomic::Ordering::SeqCst);
        dropped
    }

    pub(super) async fn trust_round(&self) -> bool {
        let fetched = crate::web_updates::fetch_trust().await;
        let dropped = self.apply_round(fetched, now() as u64);
        // Not inside `apply_round`: the statement in force is process-wide
        // state that tests of the updater must not see change.
        self.publish_trust();
        dropped
    }

    /// Shows the statement in force to the native updater.
    fn publish_trust(&self) {
        if let Ok(trust) = self.trust.lock() {
            crate::web_trust::publish(trust.statement());
        }
    }

    /// The running session of a server.
    pub(crate) fn session(&self, id: &str) -> Option<Arc<Session>> {
        self.registry.lock().ok()?.session(id)
    }

    /// The bundle new windows run: the front server's, else the newest.
    fn view(&self) -> Option<Held> {
        let front = {
            let registry = self.registry.lock().ok()?;
            registry
                .active()
                .and_then(|id| registry.session(id))
                .map(|s| s.held.clone())
        };
        match front {
            Some(held) => Some(held),
            None => self.store.lock().ok()?.newest().cloned(),
        }
    }

    pub(crate) fn instances_file(&self) -> Option<std::path::PathBuf> {
        Some(self.data_dir.as_ref()?.join(INSTANCES_FILE))
    }

    /// Writes the list of servers down (addresses only).
    pub(crate) fn save_instances(&self) {
        if let Ok(registry) = self.registry.lock() {
            registry.stored().save(self.instances_file().as_deref());
        }
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn data_dir(&self) -> Option<&std::path::Path> {
        self.data_dir.as_deref()
    }

    pub(crate) fn jar_file(&self, origin: &Url) -> Option<std::path::PathBuf> {
        let name = crate::instances::id_for(origin);
        Some(
            self.data_dir
                .as_ref()?
                .join("sessions")
                .join(format!("{name}.json")),
        )
    }
}

/// Why a server's tab stopped when its bundle was withdrawn.
const WITHDRAWN: &str =
    "The web app of this server was withdrawn. Restart the client to get a current one.";

/// What this host offers the page beyond the common surface. The web app
/// detects these instead of bumping `web_client_api`; older bundles ignore it.
/// `notify`: toasts and the badge. `unread`: the page reports unread channels
/// and mentions for the server rail. `ws_query`: the socket may ask for
/// conversation events (`/api/ws?dm=1`).
fn capabilities(role: &str) -> &'static [&'static str] {
    if role == "instance" {
        &["notify", "unread", "ws_query"]
    } else {
        &[]
    }
}

/// Announces the verified bundle to the page before any of its scripts run.
/// The app honours it only on the host's own origin. A server's page passes
/// the bundle it was given; every other window runs the one in front.
pub fn identity_script(app: &tauri::AppHandle, role: &str, held: Option<&Held>) -> String {
    let front;
    let held = match held {
        Some(held) => held,
        None => {
            let Some(view) = app.state::<Trusted>().view() else {
                return String::new();
            };
            front = view;
            &front
        }
    };
    let identity = serde_json::json!({
        "version": held.bundle.version.to_string(),
        "source": held.source.as_str(),
        "webClientApi": held.bundle.web_client_api,
        "keyId": held.bundle.key_id,
        "commit": held.bundle.commit,
        "role": role,
        "capabilities": capabilities(role),
    });
    format!(
        "if(window===window.top)Object.defineProperty(window,'__MNEMA_TRUSTED_APP__',{{value:Object.freeze({identity})}});"
    )
}

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

pub(crate) fn address(input: &str) -> Result<Url, String> {
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err("Enter an HTTPS instance address.".into());
    }
    let input = input.trim();
    let raw = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    if raw.contains('\\') || raw.chars().any(char::is_whitespace) {
        return Err("Invalid instance address.".into());
    }
    let url = Url::parse(&raw).map_err(|_| "Invalid instance address.")?;
    // HTTPS, except for an instance on this very machine (a development stack).
    let secure = url.scheme() == "https" || (url.scheme() == "http" && loopback(&url));
    if !secure
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || raw
            .split_once("://")
            .is_some_and(|(_, authority)| authority.contains('@'))
    {
        return Err("Use an HTTPS origin without credentials, a path or query.".into());
    }
    Ok(url)
}

#[derive(Deserialize)]
struct Health {
    status: String,
    #[serde(default)]
    version: String,
    web_client_api: Option<u32>,
}

fn parse_health(bytes: &[u8]) -> Result<ServerInfo, String> {
    let health: Health = serde_json::from_slice(bytes)
        .map_err(|_| "This instance needs an update for the desktop client.")?;
    match (health.status.as_str(), health.web_client_api) {
        ("ok", Some(web_client_api)) if web_client_api >= 1 => Ok(ServerInfo {
            version: Version::parse(&health.version),
            web_client_api,
        }),
        ("ok", _) => Err("This instance needs an update for the desktop client.".into()),
        _ => Err("This instance is not compatible with this desktop client.".into()),
    }
}

pub(crate) async fn discover(origin: &Url) -> Result<ServerInfo, String> {
    let client = Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "Unable to initialize HTTPS.")?;
    let mut response = client
        .get(
            origin
                .join("api/health")
                .map_err(|_| "Invalid instance address.")?,
        )
        .send()
        .await
        .map_err(|_| "Unable to reach the instance using trusted HTTPS.")?;
    if response.status() != reqwest::StatusCode::OK {
        return Err("The instance is unavailable or redirects to a different address.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Instance check failed.")?
    {
        if chunk.len() > MAX_DISCOVERY_BYTES.saturating_sub(bytes.len()) {
            return Err("Invalid instance response.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_health(&bytes)
}

/// The verified bundle for this server. When the server is ahead of every
/// bundle held, look for an update first; never fall back to the server's own
/// JavaScript.
pub(crate) async fn choose(trusted: &Trusted, server: &ServerInfo) -> Result<Held, String> {
    let first = trusted.store.lock().map_err(|_| "busy")?.select(server);
    match first {
        Ok(held) => Ok(held),
        Err(Refusal::ServerNewer) => {
            if !trusted.fresh() {
                trusted.trust_round().await;
            }
            if trusted.fresh() {
                let _ = crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await;
            }
            trusted
                .store
                .lock()
                .map_err(|_| "busy")?
                .select(server)
                .map_err(|refusal| refusal.to_string())
        }
        Err(refusal) => Err(refusal.to_string()),
    }
}

fn into_response(resp: Resp) -> tauri::http::Response<Cow<'static, [u8]>> {
    let mut response = tauri::http::Response::new(Cow::Owned(resp.body));
    *response.status_mut() = tauri::http::StatusCode::from_u16(resp.status)
        .unwrap_or(tauri::http::StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in &resp.headers {
        if let (Ok(name), Ok(value)) = (
            tauri::http::HeaderName::from_bytes(name.as_bytes()),
            tauri::http::HeaderValue::from_str(value),
        ) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

/// Why a server's page shows nothing but a sentence: it is still being
/// checked, or it was refused (the instance is too old or too new for any
/// verified bundle, or cannot be reached). Shown in that server's tab only.
fn unavailable(trusted: &Trusted, id: &str) -> Resp {
    let text = trusted
        .registry
        .lock()
        .ok()
        .and_then(|registry| match registry.slot(id)?.phase {
            Phase::Starting => Some("Connecting to the server.".to_owned()),
            Phase::Refused(ref why) => Some(why.clone()),
            Phase::Ready(_) => None,
        })
        .unwrap_or_else(|| "No instance is selected.".into());
    plain(503, &text)
}

/// One request of the page (or attachment window) of a server: its API calls
/// go to that server with that server's cookies, and its files come from the
/// bundle that server was matched with. No other server is reachable from here.
async fn page_request(trusted: &Trusted, id: &str, request: tauri::http::Request<Vec<u8>>) -> Resp {
    let Some(session) = trusted.session(id) else {
        return unavailable(trusted, id);
    };
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if path == "/api" || path.starts_with("/api/") {
        let path_and_query = request
            .uri()
            .path_and_query()
            .map_or(path.clone(), |p| p.as_str().to_owned());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        forward(
            &session.instance,
            &method,
            &path_and_query,
            &headers,
            request.into_body(),
        )
        .await
    } else {
        serve_bundle(&session.held.bundle, &method, &path)
    }
}

/// Answers one request of one webview.
async fn handle(
    app: &tauri::AppHandle,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
) -> Resp {
    let trusted = app.state::<Trusted>();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if super::web_permissions::take_probe(label, &path, request.uri().query()) {
        return plain(204, "");
    }
    let api = path == "/api" || path.starts_with("/api/");
    if let Some(id) = id_of_instance_label(label).or_else(|| id_of_attachment_label(label)) {
        return page_request(&trusted, id, request).await;
    }
    match label {
        // The probe opens an instance-labelled page before any server is
        // selected, so its document still comes from the verified bundle. API
        // calls have no server.
        "instance" if !api && super::web_permissions::probe_enabled() => match trusted.view() {
            Some(held) => serve_bundle(&held.bundle, &method, &path),
            None => plain(503, "No instance is selected."),
        },
        "instance" => plain(503, "No instance is selected."),
        // The host's own update page; independent of the web bundle.
        "updater" if !api => match super::native_updates::asset(&path) {
            Some((kind, body)) if method == "GET" || method == "HEAD" => Resp {
                status: 200,
                headers: vec![
                    ("content-type".into(), kind.into()),
                    (
                        "content-security-policy".into(),
                        super::native_updates::UI_CSP.into(),
                    ),
                    ("x-content-type-options".into(), "nosniff".into()),
                    ("cache-control".into(), "no-store".into()),
                ],
                body: body.as_bytes().to_vec(),
            },
            _ => plain(404, "Not found."),
        },
        // The server rail: the host's own page, independent of the web bundle
        // and of every server.
        "rail" if !api => super::web_shell::rail_asset(&method, &path),
        "main" | "sidepeek" | "gaming-overlay" | "gaming-settings" if !api => {
            match trusted.view() {
                Some(held) => serve_bundle(&held.bundle, &method, &path),
                None => {
                    let reason = trusted
                        .store
                        .lock()
                        .map(|s| s.empty_reason())
                        .unwrap_or(Refusal::NoBundle);
                    plain(503, &reason.to_string())
                }
            }
        }
        "main" | "rail" | "sidepeek" | "gaming-overlay" | "gaming-settings" => {
            plain(404, "Not found.")
        }
        _ => plain(403, "Forbidden."),
    }
}

fn plain(status: u16, text: &str) -> Resp {
    Resp {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("x-content-type-options".into(), "nosniff".into()),
        ],
        body: text.as_bytes().to_vec(),
    }
}

/// The server whose page made a call, if the call comes from the app's own
/// origin in the page of a server that is running. Every command that acts for
/// a server derives the server from here and from nowhere else: the label of
/// the calling webview, never an argument.
pub(super) fn instance_page(webview: &Webview) -> Result<Arc<Session>, String> {
    let id = id_of_instance_label(webview.label());
    let local = webview.url().is_ok_and(|u| local_origin(&u).is_some());
    let Some(id) = id.filter(|_| local) else {
        return Err("Only the instance window may use this.".into());
    };
    webview
        .app_handle()
        .state::<Trusted>()
        .session(id)
        .ok_or_else(|| "No instance is selected.".into())
}

#[tauri::command]
fn desktop_ws_open(
    webview: Webview,
    path: String,
    on_event: tauri::ipc::Channel<WsEvent>,
) -> Result<u64, String> {
    let session = instance_page(&webview)?;
    let dm = crate::web_ws::socket_request(&path)?;
    crate::web_ws::open(
        &session.sockets,
        Arc::clone(&session.instance),
        dm,
        move |event| on_event.send(event).is_ok(),
    )
}

#[tauri::command]
fn desktop_ws_send(webview: Webview, id: u64, data: String) -> Result<(), String> {
    instance_page(&webview)?.sockets.send(id, data)
}

#[tauri::command]
fn desktop_ws_close(webview: Webview, id: u64) -> Result<(), String> {
    instance_page(&webview)?.sockets.close(id);
    Ok(())
}

pub(super) fn allow_permission(
    webview: tauri::Webview<tauri::Wry>,
    kind: tauri::webview::PermissionKind,
) -> tauri::webview::PermissionResponse {
    let url = webview.url().ok();
    super::web_permissions::decide(webview.label(), url.as_ref(), kind)
}

/// A path the proxy serves for a server, on the app origin.
pub(super) fn app_api(url: &Url) -> bool {
    local_origin(url).is_some() && url.path().starts_with("/api/")
}

/// Verifies updates in the background; a bundle that arrives while no bundle
/// is usable (first start without a built-in one) is picked up at once.
fn refresh_in_background(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            refresh_once(&app, first).await;
            first = false;
            tokio::time::sleep(std::time::Duration::from_secs(
                crate::web_bundle::CHECK_INTERVAL_SECS,
            ))
            .await;
        }
    });
}

/// One round: the trust statement first (a revoked key must stop counting
/// before anything else is installed), then newer bundles.
async fn refresh_once(app: &tauri::AppHandle, first: bool) {
    let trusted = app.state::<Trusted>();
    if crate::web_updates::disabled() {
        return;
    }
    let dropped = trusted.trust_round().await;
    let had_bundle = trusted
        .store
        .lock()
        .map(|s| s.newest().is_some())
        .unwrap_or(false);
    let due = trusted
        .store
        .lock()
        .map(|s| s.check_due(now() as u64))
        .unwrap_or(false);
    if had_bundle && !dropped && !(due || !first) {
        return;
    }
    // Freeze protection: no new bundles without a fresh trust statement.
    let installed = if trusted.fresh() {
        crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await
    } else {
        Ok(None)
    };
    if let Ok(mut store) = trusted.store.lock() {
        store.mark_checked(now() as u64);
    }
    // Pages that were showing a withdrawn bundle, or none, move to the newly
    // installed one, or show why there is none.
    if dropped || (!had_bundle && matches!(installed, Ok(Some(_)))) {
        super::web_shell::bundles_changed(app);
    }
}

pub(super) fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(std::sync::Mutex::new(super::web_gaming::Gaming::default()))
        .register_asynchronous_uri_scheme_protocol("mnema", |ctx, request, responder| {
            let label = ctx.webview_label().to_owned();
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let uri = request.uri().to_string();
                let resp = handle(&app, &label, request).await;
                if std::env::var_os("MNEMA_DEBUG_PROTOCOL").is_some() {
                    eprintln!("[proto] {label} {uri} -> {}", resp.status);
                }
                responder.respond(into_response(resp));
            });
        })
        .invoke_handler(tauri::generate_handler![
            super::web_shell::desktop_open_instance,
            desktop_ws_open,
            desktop_ws_send,
            desktop_ws_close,
            super::web_shell::rail_state,
            super::web_shell::rail_activate,
            super::web_shell::rail_add,
            super::web_shell::rail_cancel_add,
            super::web_shell::rail_move,
            super::web_shell::rail_remove,
            super::web_shell::rail_reconnect,
            super::web_gaming::desktop_gaming_sync,
            super::web_gaming::desktop_gaming_snapshot,
            super::web_gaming::desktop_gaming_control,
            super::web_gaming::desktop_gaming_settings,
            super::native_updates::native_update_state,
            super::native_updates::native_update_action,
            super::web_notify::desktop_notify_open,
            super::web_notify::desktop_notify,
            super::web_notify::desktop_notify_permission,
            super::web_notify::desktop_notify_request_permission,
            super::web_notify::desktop_set_badge,
            super::web_notify::desktop_report_unread
        ])
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "gaming-settings" {
                let _ = super::web_gaming::open_settings(app);
            } else if id == super::native_updates::MENU_ID {
                super::native_updates::menu_clicked(app);
            } else {
                super::web_shell::menu_clicked(app, id);
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir().ok();
            let trusted = Trusted::new(data_dir, (!BUILTIN_BUNDLE.is_empty()).then_some(BUILTIN_BUNDLE));
            if let Ok(store) = trusted.store.lock() {
                for name in &store.rejected {
                    eprintln!("Mnema Desktop: refused web bundle {name}: it does not verify against the pinned keys.");
                }
            }
            app.manage(trusted);
            app.manage(super::web_notify::Notifier::default());
            super::web_notify::setup(app.handle());
            super::native_updates::setup(app.handle());
            super::web_gaming::setup(app.handle());
            super::web_shell::create(app.handle())?;
            refresh_in_background(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!());
    if result.is_err() {
        startup_error();
    }
}

fn startup_error() {
    eprintln!(
        "Mnema Desktop DEV could not start. Check that Microsoft Edge WebView2 Runtime is installed."
    );
    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut std::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                flags: u32,
            ) -> i32;
        }
        let text: Vec<u16> = "Mnema Desktop DEV could not start.\nCheck that Microsoft Edge WebView2 Runtime is installed.\nPlease report this startup failure.".encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Mnema Desktop DEV".encode_utf16().chain(Some(0)).collect();
        // SAFETY: both strings are NUL-terminated and live throughout the call.
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10);
        }
    }
}

fn external_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str().len() <= 4096
}

pub(super) fn open_external(url: &Url) {
    let allowed = external_allowed(url);
    #[cfg(not(target_os = "windows"))]
    let _ = allowed;
    #[cfg(target_os = "windows")]
    {
        if !allowed {
            return;
        }
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut std::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                flags: u32,
            ) -> i32;
        }
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> *mut std::ffi::c_void;
        }
        let text: Vec<u16> = format!("Open this link in your browser?\n\n{}", url.as_str())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let title: Vec<u16> = "Mnema Desktop DEV".encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated buffers live throughout the synchronous dialog.
        let answer =
            unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x124) };
        if answer != 6 {
            return;
        }
        let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let target: Vec<u16> = url.as_str().encode_utf16().chain(Some(0)).collect();
        // SAFETY: the target is a bounded HTTPS URL without credentials; no
        // executable arguments or command shell are supplied. Buffers stay live.
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn only_the_instance_window_announces_notifications() {
        assert_eq!(capabilities("instance"), ["notify", "unread", "ws_query"]);
        for role in ["selector", "surface", "probe", ""] {
            assert!(capabilities(role).is_empty(), "{role}");
        }
    }

    #[test]
    fn health_decides_what_the_server_is() {
        let info =
            parse_health(br#"{"status":"ok","version":"0.7.17","web_client_api":1}"#).unwrap();
        assert_eq!(
            info,
            ServerInfo {
                version: Some(Version(0, 7, 17)),
                web_client_api: 1
            }
        );
        let dev = parse_health(br#"{"status":"ok","version":"dev","web_client_api":3}"#).unwrap();
        assert_eq!(
            dev,
            ServerInfo {
                version: None,
                web_client_api: 3
            }
        );
        for raw in [
            br#"{"status":"ok","version":"0.7.0"}"#.as_slice(),
            br#"{"status":"ok","version":"0.7.0","web_client_api":0}"#,
            b"not JSON",
        ] {
            assert!(parse_health(raw).unwrap_err().contains("needs an update"));
        }
        assert!(
            parse_health(br#"{"status":"unavailable","web_client_api":1}"#)
                .unwrap_err()
                .contains("not compatible")
        );
    }

    #[test]
    fn external_links_cannot_launch_files_scripts_or_credentialed_targets() {
        assert!(external_allowed(&url(
            "https://example.invalid/page?q=value"
        )));
        for raw in [
            "file:///tmp/example",
            "javascript:alert(1)",
            "mailto:user@example.invalid",
            "https://user:password@example.invalid/",
            "http://example.invalid/",
        ] {
            assert!(!external_allowed(&url(raw)));
        }
    }

    #[test]
    fn only_explicit_https_origins_are_selectable() {
        for valid in [
            "community.example",
            "https://community.example/",
            "https://community.example:8443",
        ] {
            assert!(address(valid).is_ok());
        }
        for invalid in [
            "",
            "http://community.example",
            "http://127.0.0.1.example",
            "http://192.168.1.10:8080",
            "https://user@community.example",
            "https://community.example/path",
            "https://community.example?token=x",
            "https://community.example/#x",
            "https://community.example\\other",
            "https://community.example\n",
        ] {
            assert!(address(invalid).is_err(), "{invalid}");
        }
    }

    fn app(path: &str) -> Url {
        app_url().join(path).unwrap()
    }

    #[test]
    fn only_the_hosts_own_origin_and_its_routes_are_app_pages() {
        let id = "11111111-1111-1111-1111-111111111111";
        for path in ["", "/", "/index.html", "/login", "/admin/users"] {
            assert!(bundled(&app(path)), "{path}");
        }
        assert!(bundled(&app(&format!("/c/{id}"))));
        assert!(bundled(&app(&format!("/v/{id}/chat"))));
        for path in [
            "/other.html",
            "/api/health",
            "/admin/unknown",
            "/c/not-an-id",
        ] {
            assert!(!bundled(&app(path)), "{path}");
        }
        for raw in [
            "https://community.example/",
            "tauri://localhost/",
            "http://tauri.localhost/",
            "mnema://localhost:9000/",
            "mnema://user@localhost/",
            "mnema://evil/",
            "https://mnema.localhost.evil.example/",
        ] {
            assert!(!bundled(&url(raw)), "{raw}");
        }
        // The other engines' form of the origin is not this platform's.
        let other = if cfg!(windows) {
            "mnema://localhost/"
        } else {
            "https://mnema.localhost/"
        };
        assert!(!bundled(&url(other)));
    }

    #[test]
    fn api_paths_on_the_app_origin_are_the_proxys() {
        assert!(app_api(&app("/api/media/1")));
        assert!(!app_api(&app("/c/1")));
        assert!(!app_api(&app("/apix")));
        assert!(!app_api(&url("https://community.example/api/media/1")));
    }

    #[test]
    fn local_origins_compare_as_text_because_custom_origins_are_opaque() {
        let a = local_origin(&app("/")).unwrap();
        assert_eq!(Some(a), local_origin(&app("/c/1")));
        assert_eq!(local_origin(&url("https://community.example/")), None);
    }

    /// 2026-11-01T00:00:00Z
    const NOW: u64 = 1_793_491_200;

    #[test]
    fn only_a_statement_from_the_pinned_root_key_changes_the_keys_in_force() {
        let trusted = Trusted::new(None, None);
        let before: Vec<String> = trusted.keys().iter().map(|k| k.id.clone()).collect();
        let doc = include_bytes!("../fixtures/trust.json");
        let sig = include_bytes!("../fixtures/trust.json.sig");
        let zero = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 64]);
        assert!(trusted.apply_trust(doc, zero.as_bytes(), NOW).is_err());
        let mut tampered = doc.to_vec();
        tampered[12] ^= 1;
        assert!(trusted.apply_trust(&tampered, sig, NOW).is_err());
        // The real statement lists the pinned key: nothing is dropped.
        assert_eq!(trusted.apply_trust(doc, sig, NOW), Ok(false));
        let after: Vec<String> = trusted.keys().iter().map(|k| k.id.clone()).collect();
        assert_eq!(before, after);
        // Applying it again is a no-op.
        assert_eq!(trusted.apply_trust(doc, sig, NOW), Ok(false));
    }

    /// Starts a server on the list with a running session on `held`.
    fn run_server(trusted: &Trusted, host: &str, held: Held) -> (String, Arc<Session>) {
        run_server_at(
            trusted,
            Url::parse(&format!("https://{host}/")).unwrap(),
            held,
        )
    }

    fn run_server_at(trusted: &Trusted, origin: Url, held: Held) -> (String, Arc<Session>) {
        use crate::web_proxy::{Instance, Jar, Limits, client};
        let mut registry = trusted.registry.lock().unwrap();
        let id = registry.add(origin.clone()).unwrap();
        let session = Arc::new(Session {
            id: id.clone(),
            instance: Arc::new(Instance {
                client: client().unwrap(),
                jar: Jar::new(None, 0),
                origin,
                limits: Limits::default(),
            }),
            held,
            sockets: Arc::new(crate::web_ws::Sockets::default()),
        });
        registry.slot_mut(&id).unwrap().phase = Phase::Ready(Arc::clone(&session));
        (id, session)
    }

    fn held_of(version: &'static str, seed: u8) -> Held {
        use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
        let mut spec = Spec::new(version);
        spec.seed = seed;
        Store::load(None, Some(&bundle(&spec)), &keys(seed))
            .select(&ServerInfo {
                version: Version::parse(version),
                web_client_api: 1,
            })
            .unwrap()
    }

    #[test]
    fn a_cap_that_withdraws_the_running_bundle_ends_the_session() {
        use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
        let trusted = Trusted::new(None, None);
        let caps = |v: Version| std::collections::BTreeMap::from([(1, v)]);
        let (id, session) = {
            let mut store = trusted.store.lock().unwrap();
            *store = Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
            run_server(&trusted, "community.example", held_of("0.9.0", 7))
        };
        // A cap at the running version changes nothing.
        let mut store = trusted.store.lock().unwrap();
        assert!(!trusted.enforce(&mut store, &keys(7), &caps(Version(0, 9, 0))));
        assert!(trusted.session(&id).is_some());
        // A cap below it withdraws the bundle, and the session ends with it.
        assert!(trusted.enforce(&mut store, &keys(7), &caps(Version(0, 8, 0))));
        assert!(trusted.session(&id).is_none());
        drop(store);
        let registry = trusted.registry.lock().unwrap();
        let Phase::Refused(why) = &registry.slot(&id).unwrap().phase else {
            panic!("the server's tab must say why it stopped")
        };
        assert_eq!(why, WITHDRAWN);
        drop(registry);
        // Its sockets went with it (none were open, and none may be opened).
        assert!(session.sockets.send(1, "x".into()).is_err());
    }

    #[test]
    fn a_withdrawn_bundle_ends_only_the_sessions_that_run_it() {
        use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
        let trusted = Trusted::new(None, None);
        let (a, b) = {
            let mut store = trusted.store.lock().unwrap();
            *store = Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
            // B runs a bundle signed by a key that is not in force (revoked).
            let a = run_server(&trusted, "a.example", held_of("0.9.0", 7)).0;
            let b = run_server(&trusted, "b.example", held_of("0.9.0", 8)).0;
            (a, b)
        };
        let mut store = trusted.store.lock().unwrap();
        trusted.enforce(&mut store, &keys(7), &std::collections::BTreeMap::new());
        drop(store);
        assert!(
            trusted.session(&a).is_some(),
            "the server on a held bundle keeps running"
        );
        assert!(
            trusted.session(&b).is_none(),
            "the one on a revoked key stops"
        );
    }

    /// A server that answers `/api/login` with a session cookie named after
    /// itself, `/api/me` only for that cookie, and `/api/echo` with the cookie
    /// it was sent.
    async fn tiny_server(name: &'static str) -> Url {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 8192];
                    let n = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let cookie = request.lines().find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("cookie: ")
                            .map(str::to_owned)
                    });
                    let path = request.split(' ').nth(1).unwrap_or("").to_owned();
                    let valid = cookie.as_deref() == Some(&format!("session={name}-token"));
                    let (status, extra, body) = match path.as_str() {
                        "/api/login" => (
                            "200 OK",
                            format!("set-cookie: session={name}-token; Max-Age=3600\r\n"),
                            "{}".to_owned(),
                        ),
                        "/api/me" if valid => ("200 OK", String::new(), "{}".to_owned()),
                        "/api/me" => ("401 Unauthorized", String::new(), "{}".to_owned()),
                        _ => (
                            "200 OK",
                            String::new(),
                            serde_json::json!({"server": name, "cookie": cookie}).to_string(),
                        ),
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        origin
    }

    async fn page_call(trusted: &Trusted, id: &str, method: &str, path: &str) -> Resp {
        let request = tauri::http::Request::builder()
            .method(method)
            .uri(format!("mnema://localhost{path}"))
            .body(Vec::new())
            .unwrap();
        page_request(trusted, id, request).await
    }

    fn json(resp: &Resp) -> serde_json::Value {
        serde_json::from_slice(&resp.body).unwrap_or_default()
    }

    #[tokio::test]
    async fn the_page_of_a_server_reaches_that_server_with_that_servers_cookies_only() {
        let trusted = Trusted::new(None, None);
        let (a, _) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
        let (b, _) = run_server_at(&trusted, tiny_server("b").await, held_of("0.9.0", 7));
        // A signs in. B's page sees none of it.
        page_call(&trusted, &a, "POST", "/api/login").await;
        assert_eq!(
            json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
            "session=a-token"
        );
        let from_b = json(&page_call(&trusted, &b, "GET", "/api/echo").await);
        assert_eq!(
            (from_b["server"].as_str(), from_b["cookie"].is_null()),
            (Some("b"), true)
        );
        // B signs in: both keep their own.
        page_call(&trusted, &b, "POST", "/api/login").await;
        assert_eq!(
            json(&page_call(&trusted, &b, "GET", "/api/echo").await)["cookie"],
            "session=b-token"
        );
        assert_eq!(
            json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
            "session=a-token"
        );
        assert_eq!(page_call(&trusted, &a, "GET", "/api/me").await.status, 200);
        assert_eq!(page_call(&trusted, &b, "GET", "/api/me").await.status, 200);
    }

    #[tokio::test]
    async fn a_page_of_a_server_that_is_not_running_gets_a_sentence_and_no_other_servers_data() {
        let trusted = Trusted::new(None, None);
        let (a, _) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
        page_call(&trusted, &a, "POST", "/api/login").await;
        // A page that is not (or no longer) on the list.
        let gone = page_call(&trusted, "0000000000000000", "GET", "/api/echo").await;
        assert_eq!(gone.status, 503);
        assert_eq!(gone.body, b"No instance is selected.");
        // A starting and a refused slot say what they are doing.
        let old = trusted
            .registry
            .lock()
            .unwrap()
            .add(Url::parse("https://old.example/").unwrap())
            .unwrap();
        assert_eq!(
            page_call(&trusted, &old, "GET", "/").await.body,
            b"Connecting to the server."
        );
        trusted
            .registry
            .lock()
            .unwrap()
            .slot_mut(&old)
            .unwrap()
            .phase = Phase::Refused("This instance needs an update for the desktop client.".into());
        let refused = page_call(&trusted, &old, "GET", "/api/echo").await;
        assert_eq!(
            (refused.status, refused.body.as_slice()),
            (
                503,
                b"This instance needs an update for the desktop client.".as_slice()
            )
        );
        // The running one is unaffected.
        assert_eq!(
            json(&page_call(&trusted, &a, "GET", "/api/echo").await)["cookie"],
            "session=a-token"
        );
    }

    #[tokio::test]
    async fn after_a_removal_from_the_server_the_next_sign_in_replaces_the_stale_session() {
        // The instance ended the session (a kick, a password change, a sign-out
        // elsewhere): the stored cookie is refused until the person signs in
        // again, and the new cookie replaces it. Nothing retries by itself.
        let trusted = Trusted::new(None, None);
        let (id, session) = run_server_at(&trusted, tiny_server("a").await, held_of("0.9.0", 7));
        session
            .instance
            .jar
            .store(&["session=revoked; Max-Age=3600".into()], false, now());
        assert_eq!(page_call(&trusted, &id, "GET", "/api/me").await.status, 401);
        assert_eq!(
            page_call(&trusted, &id, "GET", "/api/me").await.status,
            401,
            "the same answer every time, no side effects"
        );
        page_call(&trusted, &id, "POST", "/api/login").await;
        assert_eq!(page_call(&trusted, &id, "GET", "/api/me").await.status, 200);
        assert_eq!(
            session.instance.jar.header(now()).as_deref(),
            Some("session=a-token")
        );
    }

    #[test]
    fn only_the_plain_owner_of_the_old_storage_inherits_it() {
        assert_eq!(decide_old_storage(true, 0), OldStorage::Adopt);
        // A server without a session of its own is not the owner.
        assert_eq!(decide_old_storage(false, 0), OldStorage::Wipe);
        // Another server was stored too: the shared storage holds both.
        assert_eq!(decide_old_storage(true, 1), OldStorage::Wipe);
        assert_eq!(decide_old_storage(false, 2), OldStorage::Wipe);
    }

    #[test]
    fn a_server_added_next_to_a_stranger_in_the_old_data_does_not_inherit_its_storage() {
        use crate::web_vault::{Vault, fake::Memory};
        let dir = std::env::temp_dir().join(format!("mnema-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let a = Url::parse("https://a.example/").unwrap();
        let b = Url::parse("https://b.example/").unwrap();
        let session = |origin: &Url| {
            dir.join("sessions")
                .join(format!("{}.json", crate::instances::id_for(origin)))
        };
        std::fs::write(session(&a), b"[]").unwrap();
        let vault = Memory::default();
        // Nothing left by an old client, or the list exists already: nothing to decide.
        let none = std::env::temp_dir().join(format!("mnema-adopt-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&none);
        let fresh = Trusted::new(Some(none.clone()), None);
        assert_eq!(fresh.old_storage_for(&a, &vault), OldStorage::None);
        let _ = std::fs::remove_dir_all(none);
        // A is the only server the old client stored: only A inherits, once.
        let sole = Trusted::new(Some(dir.clone()), None);
        assert_eq!(
            sole.old_storage_for(&b, &vault),
            OldStorage::Wipe,
            "B is not the owner"
        );
        assert_eq!(
            sole.old_storage_for(&a, &vault),
            OldStorage::None,
            "decided once"
        );
        let again = Trusted::new(Some(dir.clone()), None);
        assert_eq!(again.old_storage_for(&a, &vault), OldStorage::Adopt);
        // A and B were both stored (A's in the keychain, B's as a file): neither inherits.
        std::fs::write(session(&b), b"[]").unwrap();
        vault.save(&crate::instances::id_for(&a), b"[]");
        for first in [&a, &b] {
            let both = Trusted::new(Some(dir.clone()), None);
            assert_eq!(both.old_storage_for(first, &vault), OldStorage::Wipe);
        }
        // Once the list exists the old storage is not up for grabs any more.
        let listed = Trusted::new(Some(dir.clone()), None);
        listed.save_instances();
        let later = Trusted::new(Some(dir.clone()), None);
        assert_eq!(later.old_storage_for(&a, &vault), OldStorage::None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn installs_need_a_fresh_statement_and_a_successful_fetch_in_this_round() {
        let doc = include_bytes!("../fixtures/trust.json").to_vec();
        let sig = include_bytes!("../fixtures/trust.json.sig").to_vec();
        let expiry = crate::web_trust::parse_utc("2027-04-15T22:00:57Z").unwrap();
        let trusted = Trusted::new(None, None);
        assert!(!trusted.fresh(), "no statement yet: no installs");
        // Offline: still nothing fresh, nothing dropped.
        assert!(!trusted.apply_round(Err("offline".into()), NOW));
        assert!(!trusted.fresh());
        // The statement arrives.
        trusted.apply_round(Ok((doc.clone(), sig.clone())), NOW);
        assert!(trusted.fresh());
        let keys = trusted.keys();
        // A later failed fetch keeps the last good statement, but installs stop.
        trusted.apply_round(Err("blocked".into()), NOW);
        assert!(!trusted.fresh());
        assert_eq!(trusted.keys().len(), keys.len());
        // The same statement is fresh again once fetched.
        trusted.apply_round(Ok((doc.clone(), sig.clone())), NOW);
        assert!(trusted.fresh());
        // Past its expiry the replay is refused, installs stay off and the
        // keys of the expired statement stay in force.
        trusted.apply_round(Ok((doc, sig)), expiry + 1);
        assert!(!trusted.fresh());
        assert_eq!(trusted.keys().len(), keys.len());
    }

    #[test]
    fn a_client_without_any_bundle_says_so_instead_of_loading_server_code() {
        let trusted = Trusted::new(None, None);
        assert!(trusted.view().is_none());
        assert!(trusted.session("0000000000000000").is_none());
        assert_eq!(trusted.jar_file(&url("https://a.example")), None);
        let dir = std::env::temp_dir().join("mnema-trusted-test");
        let with_dir = Trusted::new(Some(dir.clone()), None);
        let file = with_dir.jar_file(&url("https://a.example/")).unwrap();
        assert!(file.starts_with(dir.join("sessions")));
        assert_ne!(file, with_dir.jar_file(&url("https://b.example/")).unwrap());
    }

    #[test]
    fn responses_keep_their_status_and_headers() {
        let resp = into_response(Resp {
            status: 206,
            headers: vec![
                ("content-type".into(), "video/mp4".into()),
                ("bad header".into(), "x".into()),
            ],
            body: b"abc".to_vec(),
        });
        assert_eq!(resp.status(), 206);
        assert_eq!(resp.body().as_ref(), b"abc");
        assert_eq!(resp.headers()["content-type"], "video/mp4");
        assert_eq!(
            resp.headers().len(),
            1,
            "an invalid header name is dropped, not the response"
        );
        assert_eq!(into_response(plain(404, "x")).status(), 404);
        assert_eq!(
            into_response(Resp {
                status: 99,
                headers: vec![],
                body: vec![]
            })
            .status(),
            500
        );
    }
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports these rules to TypeScript (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::*;
    use serde_json::Value;

    fn load() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/client.json"))
            .expect("the vector file is JSON")
    }

    #[test]
    fn addresses_health_links_and_routes() {
        let v = load();
        for c in v["addresses"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            match address(input) {
                Ok(url) => assert_eq!(Some(url.as_str()), c["href"].as_str(), "{input:?}"),
                Err(message) => {
                    assert_eq!(Some(message.as_str()), c["error"].as_str(), "{input:?}")
                }
            }
        }
        for c in v["healths"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            match parse_health(input.as_bytes()) {
                Ok(info) => {
                    let ok = &c["ok"];
                    assert_eq!(
                        info.version.map(|v| v.to_string()).as_deref(),
                        ok["version"].as_str(),
                        "{input}"
                    );
                    assert_eq!(
                        info.web_client_api as u64,
                        ok["api"].as_u64().unwrap(),
                        "{input}"
                    );
                }
                Err(message) => assert_eq!(Some(message.as_str()), c["error"].as_str(), "{input}"),
            }
        }
        for c in v["externals"].as_array().unwrap() {
            let url = Url::parse(c["url"].as_str().unwrap()).unwrap();
            assert_eq!(
                external_allowed(&url),
                c["allowed"].as_bool().unwrap(),
                "{url}"
            );
        }
        for c in v["app_paths"].as_array().unwrap() {
            let path = c["path"].as_str().unwrap();
            assert_eq!(
                crate::local_route::app_path(path),
                c["ok"].as_bool().unwrap(),
                "{path:?}"
            );
        }
    }
}
