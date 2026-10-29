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
use crate::web_trust::{Accepted, RELEASE_FLOOR, TrustState, root_key};
use crate::web_ws::WsEvent;
use reqwest::redirect::Policy;
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
    /// The list of servers exists but could not be read (see `Stored`): the
    /// storage of the servers on it must not be taken for orphaned.
    list_unreadable: bool,
    /// One network round (trust statement, then bundle check) at a time. Holds
    /// when the last round ended, so servers that start together share it.
    round: tokio::sync::Mutex<Option<std::time::Instant>>,
}

/// How long the outcome of a round stands for servers that start right after.
const ROUND_REUSE: Duration = Duration::from_secs(60);

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
        let trust = TrustState::load_with(
            data_dir.as_ref().map(|d| d.join("web-trust")).as_deref(),
            root_key(),
            RELEASE_FLOOR,
        );
        crate::web_trust::publish(trust.statement());
        let keys = trust.effective_keys(trusted_keys());
        let store = Store::load(
            data_dir.as_ref().map(|d| d.join("web-bundles")).as_deref(),
            builtin,
            &keys,
        );
        let stored = Stored::load(data_dir.as_ref().map(|d| d.join(INSTANCES_FILE)).as_deref());
        let registry = Registry::restore(&stored);
        Trusted {
            list_unreadable: stored.unreadable(),
            store: Mutex::new(store),
            keys: Mutex::new(keys),
            fresh: std::sync::atomic::AtomicBool::new(false),
            trust: Mutex::new(trust),
            registry: Mutex::new(registry),
            adoptable: std::sync::atomic::AtomicBool::new(adoptable),
            data_dir,
            round: tokio::sync::Mutex::new(None),
        }
    }

    /// Whether the storage the single-server client left is still to be wiped.
    /// True once: on the first start of this version, before any server of
    /// this client exists. That client kept every server it ever showed in one
    /// shared storage and recorded none of them, so the storage can belong to
    /// no one server and is never handed on; everyone starts with a storage
    /// of their own and signs in again (a new device key for end-to-end media;
    /// messages live on the server). Writing the list of servers ends it.
    pub(crate) fn take_old_storage_wipe(&self) -> bool {
        self.adoptable
            .swap(false, std::sync::atomic::Ordering::SeqCst)
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
            if !holds(store, &session.held) {
                session.sockets.close_all();
                if let Some(slot) = registry.slot_mut(&id) {
                    slot.phase = Phase::Refused(WITHDRAWN.into());
                }
            }
        }
        dropped
    }

    /// Ends a session that was registered on a bundle withdrawn in the
    /// meantime. `enforce` only sees sessions already in the registry, so a
    /// server that was still starting when the key was revoked has to look
    /// again once it is registered. Either `enforce` ran before (the store no
    /// longer holds the bundle) or it runs after (it finds the session).
    /// Returns whether the session may run.
    pub(crate) fn keep_if_held(&self, id: &str, session: &Session) -> bool {
        let held = self
            .store
            .lock()
            .is_ok_and(|store| holds(&store, &session.held));
        if !held {
            session.sockets.close_all();
            if let Ok(mut registry) = self.registry.lock()
                && let Some(slot) = registry.slot_mut(id)
            {
                slot.phase = Phase::Refused(WITHDRAWN.into());
            }
        }
        held
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
        if let Ok(registry) = self.registry.lock()
            && let Err(why) = registry.stored().save(self.instances_file().as_deref())
        {
            eprintln!("Mnema Desktop: the list of servers could not be saved: {why}");
        }
    }

    /// Whether the list of servers was unusable at start, so that the storage
    /// of servers missing from it may still belong to the person's servers.
    pub(crate) fn list_unreadable(&self) -> bool {
        self.list_unreadable
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

/// Whether the store still holds the very bundle (key and version) a session runs.
fn holds(store: &Store, running: &Held) -> bool {
    store.held().iter().any(|h| {
        h.bundle.key_id == running.bundle.key_id && h.bundle.version == running.bundle.version
    })
}

/// Why a server's tab stopped when its bundle was withdrawn.
pub(crate) const WITHDRAWN: &str =
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

/// One layer of quotes or angle brackets, as a link arrives from a chat message.
fn unwrap_paste(input: &str) -> &str {
    let input = input.trim();
    let bytes = input.as_bytes();
    if bytes.len() >= 2 {
        let (open, close) = (bytes[0], bytes[bytes.len() - 1]);
        let wrapped = (open == b'<' && close == b'>')
            || (open == b'"' && close == b'"')
            || (open == b'\'' && close == b'\'');
        if wrapped {
            return input[1..input.len() - 1].trim();
        }
    }
    input
}

/// The pasted text as a URL, before the page (path, query, fragment) is removed.
fn parsed(input: &str) -> Result<Url, String> {
    if input.len() > 2048 || input.chars().any(char::is_control) {
        return Err("Enter an HTTPS instance address.".into());
    }
    let input = unwrap_paste(input);
    let raw = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    if raw.contains('\\') || raw.chars().any(char::is_whitespace) {
        return Err("Invalid instance address.".into());
    }
    let url = Url::parse(&raw).map_err(|_| "Invalid instance address.")?;
    let credentials = !url.username().is_empty()
        || url.password().is_some()
        || raw
            .split_once("://")
            .is_some_and(|(_, authority)| authority.contains('@'));
    if url.host_str().is_none() {
        return Err("Invalid instance address.".into());
    }
    if credentials || (url.scheme() != "https" && url.scheme() != "http") {
        return Err("Use an HTTPS origin without credentials, a path or query.".into());
    }
    Ok(url)
}

pub(crate) fn address(input: &str) -> Result<Url, String> {
    let mut url = parsed(input)?;
    // A pasted invite link keeps the host and an explicit port. The path,
    // query and fragment belong to the page. Plain http elsewhere becomes
    // https; a server on this machine stays http.
    if url.scheme() == "http" && !loopback(&url) {
        url.set_scheme("https")
            .map_err(|_| "Invalid instance address.".to_string())?;
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn invite_token(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches(['.', ',', ';', ')']);
    crate::local_route::invite_segment(value).then(|| value.to_owned())
}

fn path_invite(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/invite/")?;
    let code = rest.strip_suffix('/').unwrap_or(rest);
    if code.contains('/') {
        return None;
    }
    invite_token(code)
}

fn query_invite(query: Option<&str>) -> Option<String> {
    let query = query?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "invite")
        .and_then(|(_, value)| invite_token(&value))
}

fn fragment_invite(fragment: Option<&str>) -> Option<String> {
    let query = fragment?.split_once('?')?.1;
    query_invite(Some(query))
}

/// The invite code in a pasted link: `/invite/{code}`, `?invite=`, or the same
/// pair in the fragment. Anything else is not an invite, and connecting still
/// works. The code is the web app's, so the join screen can read it.
pub(crate) fn invite_code(input: &str) -> Option<String> {
    let url = parsed(input).ok()?;
    path_invite(url.path())
        .or_else(|| query_invite(url.query()))
        .or_else(|| fragment_invite(url.fragment()))
}

/// Where a new server page opens. A recognised invite opens the join screen;
/// a bare address opens the app. A code that is not an invite stays at the root.
pub(crate) fn join_page(code: Option<&str>) -> Url {
    let mut url = app_url();
    if let Some(code) = code.and_then(invite_token) {
        url.set_path(&format!("/invite/{code}"));
    }
    url
}

#[derive(Deserialize)]
struct Health {
    status: String,
    #[serde(default)]
    version: String,
    web_client_api: Option<u32>,
}

fn parse_health(bytes: &[u8]) -> Result<ServerInfo, String> {
    let health: Health = crate::web_json::parse(bytes)
        .ok_or("This instance needs an update for the desktop client.")?;
    match (health.status.as_str(), health.web_client_api) {
        ("ok", Some(web_client_api)) if web_client_api >= 1 => Ok(ServerInfo {
            version: Version::parse(&health.version),
            web_client_api,
        }),
        ("ok", _) => Err("This instance needs an update for the desktop client.".into()),
        _ => Err("This instance is not compatible with this desktop client.".into()),
    }
}

/// What the server calls itself in `/api/health` (`community_name`: the name
/// its Owner chose, else its host). It is only ever shown as text, so it is
/// cleaned and capped here; `None` for a server that sends none (older).
pub(crate) fn parse_community_name(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Named {
        community_name: Option<String>,
    }
    let named: Named = crate::web_json::parse(bytes)?;
    clean_name(&named.community_name?)
}

/// Longest name taken over; the server's own limit is 60 for a chosen name,
/// a host fallback may be longer.
const MAX_NAME_CHARS: usize = 80;

fn clean_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name: String = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Asks the server what it is: its health and the name it gives itself.
pub(crate) async fn discover(origin: &Url) -> Result<(ServerInfo, Option<String>), String> {
    let bytes = fetch_health(origin).await?;
    Ok((parse_health(&bytes)?, parse_community_name(&bytes)))
}

async fn fetch_health(origin: &Url) -> Result<Vec<u8>, String> {
    let client = crate::web_proxy::client_builder()
        .redirect(Policy::none())
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
    Ok(bytes)
}

/// The verified bundle for this server. When the server is ahead of every
/// bundle held, look for an update first; never fall back to the server's own
/// JavaScript.
pub(crate) async fn choose(trusted: &Trusted, server: &ServerInfo) -> Result<Held, String> {
    let first = trusted.store.lock().map_err(|_| "busy")?.select(server);
    match first {
        Ok(held) => Ok(held),
        Err(Refusal::ServerNewer) => {
            // Servers that start together queue here for one round. Whoever
            // comes next first looks at what the round before it brought, and
            // does not fetch again within `ROUND_REUSE`, offline or not.
            let mut last = trusted.round.lock().await;
            let select = || {
                trusted
                    .store
                    .lock()
                    .map_err(|_| "busy".to_owned())?
                    .select(server)
                    .map_err(|refusal| refusal.to_string())
            };
            if let Ok(held) = select() {
                return Ok(held);
            }
            if last.is_none_or(|at| at.elapsed() >= ROUND_REUSE) {
                if !trusted.fresh() {
                    trusted.trust_round().await;
                }
                if trusted.fresh() {
                    let _ =
                        crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await;
                }
                *last = Some(std::time::Instant::now());
            }
            select()
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
    let path = request.uri().path().to_owned();
    let Some(session) = trusted.session(id) else {
        let answer = unavailable(trusted, id);
        return if path == "/api" || path.starts_with("/api/") {
            answer.sealed()
        } else {
            answer
        };
    };
    let method = request.method().as_str().to_owned();
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

/// Whether a round really looked for a bundle. A round that was skipped
/// (no fresh trust statement) or failed (offline, refused) did not, and must
/// not postpone the next check.
fn looked(result: Option<&Result<Option<Version>, String>>) -> bool {
    result.is_some_and(Result::is_ok)
}

/// One round: the trust statement first (a revoked key must stop counting
/// before anything else is installed), then newer bundles. The statement is
/// fetched even when the update check is switched off: that switch spares the
/// release request to GitHub, and must not stop a revoked key from counting.
async fn refresh_once(app: &tauri::AppHandle, first: bool) {
    let trusted = app.state::<Trusted>();
    let mut last = trusted.round.lock().await;
    let dropped = trusted.trust_round().await;
    let updates = !crate::web_updates::disabled();
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
    let installed = if updates && (!had_bundle || dropped || due || !first) {
        // Freeze protection: no new bundles without a fresh trust statement.
        let result = if trusted.fresh() {
            Some(crate::web_updates::check_and_install(&trusted.store, &trusted.keys).await)
        } else {
            None
        };
        if looked(result.as_ref())
            && let Ok(mut store) = trusted.store.lock()
        {
            store.mark_checked(now() as u64);
        }
        *last = Some(std::time::Instant::now());
        result
    } else {
        None
    };
    drop(last);
    // Pages that were showing a withdrawn bundle, or none, move to the newly
    // installed one, or show why there is none.
    if dropped || (!had_bundle && matches!(installed, Some(Ok(Some(_))))) {
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
            super::web_shell::rail_notify_get,
            super::web_shell::rail_notify_set,
            super::web_gaming::desktop_gaming_sync,
            super::web_gaming::desktop_gaming_snapshot,
            super::web_gaming::desktop_gaming_control,
            super::web_gaming::desktop_gaming_settings,
            super::web_gaming::desktop_gaming_candidate,
            super::web_gaming::desktop_gaming_add_game,
            super::web_gaming::desktop_gaming_remove_game,
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
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(|app, event| {
                // The Dock icon brings back a window that was closed on macOS.
                #[cfg(target_os = "macos")]
                if let tauri::RunEvent::Reopen { .. } = &event {
                    super::web_shell::reopen(app);
                }
                let _ = (app, event);
            });
        });
    if result.is_err() {
        startup_error();
    }
}

/// What the person is told when the window system cannot start, and what to
/// check first on this platform.
fn startup_message() -> String {
    #[cfg(windows)]
    let hint = "Check that Microsoft Edge WebView2 Runtime is installed.";
    #[cfg(not(windows))]
    let hint = "Check that macOS is up to date and that the data folder of the app is writable.";
    format!("{APP_TITLE} could not start.\n{hint}\nPlease report this startup failure.")
}

fn startup_error() {
    let message = startup_message();
    eprintln!("{}", message.replace('\n', " "));
    show_dialog(Dialog::Error, &message);
}

fn external_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str().len() <= 4096
}

/// Asks before a link leaves the app, then hands it to the system browser.
pub(super) fn open_external(url: &Url) {
    if !external_allowed(url) {
        return;
    }
    #[cfg(target_os = "windows")]
    if show_dialog(Dialog::Confirm, &confirm_text(url)) {
        windows_dialog::open(url.as_str());
    }
    // The dialog waits for an answer: not on the thread that runs the window.
    #[cfg(target_os = "macos")]
    {
        let url = url.clone();
        std::thread::spawn(move || {
            if show_dialog(Dialog::Confirm, &confirm_text(&url)) {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(url.as_str())
                    .status();
            }
        });
    }
}

fn confirm_text(url: &Url) -> String {
    format!("Open this link in your browser?\n\n{}", url.as_str())
}

/// The name the host shows in its own dialogs.
const APP_TITLE: &str = "Mnema Desktop DEV";

#[derive(Clone, Copy)]
enum Dialog {
    /// Something went wrong; one button.
    Error,
    /// A question; the default answer is no. Returns whether it was yes.
    Confirm,
}

/// A modal dialog of the platform. `true` unless a question was answered no
/// (or could not be asked).
fn show_dialog(kind: Dialog, text: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_dialog::show(kind, text)
    }
    #[cfg(target_os = "macos")]
    {
        // The text goes in as an argument of the script, never into its source.
        let status = std::process::Command::new("/usr/bin/osascript")
            .args(osascript_args(kind, text))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match kind {
            Dialog::Error => true,
            Dialog::Confirm => status.is_ok_and(|s| s.success()),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (kind, text);
        false
    }
}

/// The argument vector of `osascript` for a dialog: the script lines, then the
/// text and the title as its `argv`. Cancel makes `osascript` fail.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn osascript_args(kind: Dialog, text: &str) -> Vec<String> {
    let line = match kind {
        Dialog::Error => "display alert (item 2 of argv) message (item 1 of argv) as critical",
        Dialog::Confirm => {
            "display dialog (item 1 of argv) with title (item 2 of argv) buttons {\"Cancel\", \"Open\"} default button \"Cancel\" cancel button \"Cancel\" with icon caution"
        }
    };
    ["-e", "on run argv", "-e", line, "-e", "end run"]
        .into_iter()
        .map(str::to_owned)
        .chain([text.to_owned(), APP_TITLE.to_owned()])
        .collect()
}

#[cfg(target_os = "windows")]
mod windows_dialog {
    use super::{APP_TITLE, Dialog};

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

    /// NUL-terminated UTF-16.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn show(kind: Dialog, text: &str) -> bool {
        // MB_OK | MB_ICONERROR; MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2.
        let flags = match kind {
            Dialog::Error => 0x10,
            Dialog::Confirm => 0x124,
        };
        let (text, title) = (wide(text), wide(APP_TITLE));
        // SAFETY: both strings are NUL-terminated and live throughout the call.
        let answer =
            unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags) };
        match kind {
            Dialog::Error => true,
            Dialog::Confirm => answer == 6,
        }
    }

    pub(super) fn open(target: &str) {
        let (operation, target) = (wide("open"), wide(target));
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
    fn the_name_a_server_gives_itself_is_cleaned_before_it_is_shown() {
        let name = |body: &str| parse_community_name(body.as_bytes());
        let ok = r#""status":"ok","version":"1.3.0","web_client_api":1"#;
        assert_eq!(
            name(&format!(r#"{{{ok},"community_name":"  Lantern\n Fox "}}"#)).as_deref(),
            Some("Lantern Fox")
        );
        assert_eq!(
            name(&format!(r#"{{{ok}}}"#)),
            None,
            "older servers send none"
        );
        assert_eq!(name(&format!(r#"{{{ok},"community_name":"   "}}"#)), None);
        assert_eq!(name(&format!(r#"{{{ok},"community_name":7}}"#)), None);
        assert_eq!(
            name(&format!(
                r#"{{{ok},"community_name":"{}"}}"#,
                "x".repeat(200)
            ))
            .map(|n| n.chars().count()),
            Some(80)
        );
        assert_eq!(name("not JSON"), None);
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
    fn an_address_keeps_its_port_and_drops_a_pasted_invite_link() {
        for valid in [
            "community.example",
            "https://community.example/",
            "https://community.example:8443",
        ] {
            assert!(address(valid).is_ok());
        }
        assert_eq!(
            address("https://community.example:8443/?invite=code")
                .unwrap()
                .as_str(),
            "https://community.example:8443/"
        );
        assert_eq!(
            address("http://community.example:8443/path?invite=code")
                .unwrap()
                .as_str(),
            "https://community.example:8443/"
        );
        assert_eq!(
            address("http://localhost:8080/path?x=1").unwrap().as_str(),
            "http://localhost:8080/"
        );
        assert_eq!(
            address("<https://community.example:8443/>")
                .unwrap()
                .as_str(),
            "https://community.example:8443/"
        );
        for invalid in [
            "",
            "https://user@community.example",
            "https://community.example\\other",
            "https://community.example\n",
        ] {
            assert!(address(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_pasted_invite_opens_the_join_screen_and_a_bare_address_does_not() {
        assert_eq!(invite_code("community.example:8443"), None);
        assert_eq!(
            invite_code("https://community.example:8443/?invite=abc234xyz789").as_deref(),
            Some("abc234xyz789")
        );
        assert_eq!(
            invite_code("http://community.example:8443/invite/Team_Night-2026/").as_deref(),
            Some("Team_Night-2026")
        );
        assert_eq!(
            invite_code("https://community.example/#/join?invite=abc234xyz789").as_deref(),
            Some("abc234xyz789")
        );
        assert_eq!(
            invite_code("https://community.example:8443/?invite=abc234xyz789.").as_deref(),
            Some("abc234xyz789")
        );
        assert_eq!(
            invite_code("<https://community.example/?invite=abc234xyz789>").as_deref(),
            Some("abc234xyz789")
        );
        assert_eq!(invite_code("https://community.example/?invite=short"), None);
        assert_eq!(
            invite_code("https://user@community.example/?invite=abc234xyz789"),
            None
        );
        assert_eq!(
            join_page(Some("abc234xyz789")).path(),
            "/invite/abc234xyz789"
        );
        assert_eq!(join_page(None).path(), "/");
        assert_eq!(join_page(Some("../x")).path(), "/");
        assert_eq!(
            address("https://community.example:8443/invite/abc234xyz789")
                .unwrap()
                .as_str(),
            "https://community.example:8443/"
        );
    }

    fn app(path: &str) -> Url {
        app_url().join(path).unwrap()
    }

    #[test]
    fn only_the_hosts_own_origin_and_its_routes_are_app_pages() {
        let id = "11111111-1111-1111-1111-111111111111";
        for path in [
            "",
            "/",
            "/index.html",
            "/login",
            "/admin/users",
            "/invite/abc234xyz789",
        ] {
            assert!(bundled(&app(path)), "{path}");
        }
        assert!(bundled(&app(&format!("/c/{id}"))));
        assert!(bundled(&app(&format!("/v/{id}/chat"))));
        // Channels and conversations are addressed by slug and name.
        for path in [
            "/c/general",
            "/v/lounge/chat",
            "/dm/alice",
            "/dm/g/1a2b3c4d",
        ] {
            assert!(bundled(&app(path)), "{path}");
        }
        for path in [
            "/other.html",
            "/api/health",
            "/admin/unknown",
            "/c/Not-A-Slug",
            "/invite/abc",
            "/invite/abc234xyz789/extra",
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
            community: None,
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
    fn dialogs_pass_their_text_as_arguments_and_name_the_platforms_own_dependency() {
        let link = url("https://example.invalid/a?q=\"x\"&r='y'");
        let text = confirm_text(&link);
        for kind in [Dialog::Error, Dialog::Confirm] {
            let args = osascript_args(kind, &text);
            // The text is data for the script, never part of its source.
            assert_eq!(
                &args[args.len() - 2..],
                [text.clone(), APP_TITLE.to_owned()]
            );
            assert!(
                args[..args.len() - 2]
                    .iter()
                    .all(|a| !a.contains("example.invalid"))
            );
        }
        let message = startup_message();
        assert!(message.starts_with(APP_TITLE));
        assert_eq!(message.contains("WebView2"), cfg!(windows));
    }

    #[test]
    fn a_round_that_did_not_look_for_a_bundle_is_not_a_completed_check() {
        assert!(
            !looked(None),
            "no fresh trust statement: nothing was checked"
        );
        assert!(!looked(Some(&Err("unreachable".to_owned()))));
        assert!(looked(Some(&Ok(None))));
        assert!(looked(Some(&Ok(Some(Version(1, 0, 0))))));
    }

    #[test]
    fn a_session_registered_after_its_bundle_was_withdrawn_is_ended() {
        use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
        let trusted = Trusted::new(None, None);
        // The bundle was chosen while key 7 was in force; the store lost it
        // (revoked) before the session reached the registry.
        let held = held_of("0.9.0", 7);
        *trusted.store.lock().unwrap() =
            Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(8));
        let (id, session) = run_server(&trusted, "late.example", held);
        assert!(!trusted.keep_if_held(&id, &session));
        assert!(trusted.session(&id).is_none());
        let registry = trusted.registry.lock().unwrap();
        let Phase::Refused(why) = &registry.slot(&id).unwrap().phase else {
            panic!("the tab must say why it stopped")
        };
        assert_eq!(why, WITHDRAWN);
        drop(registry);
        assert!(session.sockets.send(1, "x".into()).is_err());
        // A held bundle stays.
        *trusted.store.lock().unwrap() =
            Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
        let (other, running) = run_server(&trusted, "ok.example", held_of("0.9.0", 7));
        assert!(trusted.keep_if_held(&other, &running));
        assert!(trusted.session(&other).is_some());
    }

    #[tokio::test]
    async fn servers_that_start_together_share_one_network_round() {
        use crate::web_bundle::fixture::{Spec, bundle, trusted as keys};
        let trusted = Trusted::new(None, None);
        *trusted.store.lock().unwrap() =
            Store::load(None, Some(&bundle(&Spec::new("0.9.0"))), &keys(7));
        let newer = ServerInfo {
            version: Version::parse("9.0.0"),
            web_client_api: 99,
        };
        assert!(trusted.store.lock().unwrap().select(&newer).is_err());
        // A round ended a moment ago: nobody fetches again, offline or not.
        let at = std::time::Instant::now();
        *trusted.round.lock().await = Some(at);
        let started = std::time::Instant::now();
        let (a, b, c) = tokio::join!(
            choose(&trusted, &newer),
            choose(&trusted, &newer),
            choose(&trusted, &newer)
        );
        assert!(a.is_err() && b.is_err() && c.is_err());
        assert_eq!(*trusted.round.lock().await, Some(at));
        assert!(!trusted.fresh(), "no trust fetch ran");
        assert!(started.elapsed() < Duration::from_secs(5));
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
    fn the_old_storage_is_wiped_once_and_never_handed_on() {
        let dir = std::env::temp_dir().join(format!("mnema-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        // Sessions of two servers, whoever they were: nothing proves who owned the storage.
        for host in ["a.example", "b.example"] {
            let origin = Url::parse(&format!("https://{host}/")).unwrap();
            std::fs::write(
                dir.join("sessions")
                    .join(format!("{}.json", crate::instances::id_for(&origin))),
                b"[]",
            )
            .unwrap();
        }
        // The first start of this version: the wipe is due, once.
        let first = Trusted::new(Some(dir.clone()), None);
        assert!(first.take_old_storage_wipe());
        assert!(!first.take_old_storage_wipe(), "once per start");
        // Writing the list is what marks it done: no second wipe on any later start.
        first.save_instances();
        assert!(!Trusted::new(Some(dir.clone()), None).take_old_storage_wipe());
        assert!(!Trusted::new(Some(dir.clone()), None).take_old_storage_wipe());
        // A fresh install has nothing to wipe.
        let none = std::env::temp_dir().join(format!("mnema-adopt-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&none);
        assert!(!Trusted::new(Some(none.clone()), None).take_old_storage_wipe());
        // And no data directory, no wipe.
        assert!(!Trusted::new(None, None).take_old_storage_wipe());
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(none);
    }

    /// What the shipped client's capabilities grant is exactly what it
    /// registers: a permission without a handler in this build is a grant
    /// nobody reviewed, and a handler list that grows later must grow the
    /// capability files on purpose. Webviews are named by label, never by window.
    #[test]
    fn every_granted_permission_has_a_handler_in_this_build_and_no_capability_names_a_window() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = include_str!("web_client.rs");
        let start = source.find("tauri::generate_handler![").unwrap();
        let handlers = &source[start..start + source[start..].find("])").unwrap()];
        let registered: Vec<&str> = handlers
            .lines()
            .skip(1)
            .map(|l| l.trim().trim_end_matches(',').rsplit("::").next().unwrap())
            .filter(|n| !n.is_empty())
            .collect();
        assert!(registered.contains(&"desktop_open_instance"));
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("tauri.conf.json")).unwrap())
                .unwrap();
        let listed: Vec<&str> = config["app"]["security"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c.as_str())
            .collect();
        for entry in std::fs::read_dir(root.join("capabilities")).unwrap() {
            let path = entry.unwrap().path();
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let id = json["identifier"].as_str().unwrap();
            assert!(listed.contains(&id), "{id} is not in tauri.conf.json");
            assert!(json.get("windows").is_none(), "{id} names windows");
            for permission in json["permissions"].as_array().unwrap() {
                let command = permission
                    .as_str()
                    .unwrap()
                    .strip_prefix("allow-")
                    .unwrap()
                    .replace('-', "_");
                assert!(
                    registered.contains(&command.as_str()),
                    "{id} grants {command}, which this build does not register"
                );
            }
        }
        assert_eq!(
            listed.len(),
            std::fs::read_dir(root.join("capabilities"))
                .unwrap()
                .count()
        );
    }

    #[test]
    fn an_unusable_list_of_servers_stops_the_sweep_of_their_storage() {
        let dir = std::env::temp_dir().join(format!("mnema-list-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!Trusted::new(Some(dir.clone()), None).list_unreadable());
        std::fs::write(
            dir.join(INSTANCES_FILE),
            r#"{"version":2,"servers":[],"active":null}"#,
        )
        .unwrap();
        let trusted = Trusted::new(Some(dir.clone()), None);
        assert!(trusted.list_unreadable());
        assert!(trusted.registry.lock().unwrap().ids().is_empty());
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

    /// The same health document in forms a strict reader refuses: byte order
    /// mark, repeated keys, wrong types, data after the document.
    #[test]
    fn health_json() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let v: Value = serde_json::from_str(include_str!("../../../test-vectors/json.json"))
            .expect("the vector file is JSON");
        for c in v["healths"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let bytes = STANDARD.decode(c["input"].as_str().unwrap()).unwrap();
            match parse_health(&bytes) {
                Ok(info) => {
                    let ok = &c["result"]["ok"];
                    assert_eq!(
                        info.version.map(|v| v.to_string()).as_deref(),
                        ok["version"].as_str(),
                        "{name}"
                    );
                    assert_eq!(
                        Some(u64::from(info.web_client_api)),
                        ok["api"].as_u64(),
                        "{name}"
                    );
                }
                Err(message) => assert_eq!(
                    Some(message.as_str()),
                    c["result"]["error"].as_str(),
                    "{name}"
                ),
            }
        }
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
        for c in v["invites"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            assert_eq!(
                invite_code(input).as_deref(),
                c["code"].as_str(),
                "{input:?}"
            );
        }
    }
}
