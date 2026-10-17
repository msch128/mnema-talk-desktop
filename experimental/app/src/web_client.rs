//! The trusted desktop client. The web app runs from a signed bundle that this
//! host verified, at the host's own origin; the selected instance is reached
//! through the host (`web_proxy`, `web_ws`), so it supplies data and never
//! code. See mnema-talk `doc/desktop-trusted-client.md`.
use crate::web_bundle::{Held, Refusal, ServerInfo, Store, TrustedKey, Version, trusted_keys};
use crate::web_proxy::{Instance, Jar, Resp, client, forward, now, serve_bundle};
use crate::web_trust::{Accepted, TrustState, root_key};
use crate::web_ws::{Sockets, WsEvent};
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

const MAX_DISCOVERY_BYTES: usize = 16 * 1024;

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

/// The state of the trusted client: verified bundles, the selected instance
/// and its sockets.
pub struct Trusted {
    store: Mutex<Store>,
    /// Bundle keys in force: the trust statement's, else the compiled-in ones.
    keys: Mutex<Vec<TrustedKey>>,
    trust: Mutex<TrustState>,
    /// A fresh trust statement is in force and this round's fetch succeeded.
    /// Without it nothing new is installed (freeze protection); the already
    /// verified bundles keep running.
    fresh: std::sync::atomic::AtomicBool,
    session: Mutex<Option<Arc<Session>>>,
    sockets: Arc<Sockets>,
    data_dir: Option<std::path::PathBuf>,
}

pub struct Session {
    instance: Arc<Instance>,
    held: Held,
}

impl Trusted {
    pub(crate) fn new(data_dir: Option<std::path::PathBuf>, builtin: Option<&[u8]>) -> Trusted {
        let trust = TrustState::load(
            data_dir.as_ref().map(|d| d.join("web-trust")).as_deref(),
            root_key(),
        );
        crate::web_trust::publish(trust.statement());
        let keys = trust.effective_keys(trusted_keys());
        let fresh = trust.fresh(now() as u64);
        let store = Store::load(
            data_dir.as_ref().map(|d| d.join("web-bundles")).as_deref(),
            builtin,
            &keys,
        );
        Trusted {
            store: Mutex::new(store),
            keys: Mutex::new(keys),
            fresh: std::sync::atomic::AtomicBool::new(fresh),
            trust: Mutex::new(trust),
            session: Mutex::new(None),
            sockets: Arc::new(Sockets::default()),
            data_dir,
        }
    }

    #[cfg(test)]
    fn keys(&self) -> Vec<TrustedKey> {
        self.keys.lock().map(|k| k.clone()).unwrap_or_default()
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
        let keys = {
            let mut trust = self.trust.lock().map_err(|_| "busy")?;
            if trust.accept(doc, sig, now)? == Accepted::Unchanged {
                return Ok(false);
            }
            trust.effective_keys(trusted_keys())
        };
        *self.keys.lock().map_err(|_| "busy")? = keys.clone();
        let dropped = store.reconcile(&keys);
        drop(store);
        if dropped
            && let Some(session) = self.session()
            && !keys.iter().any(|k| k.id == session.held.bundle.key_id)
        {
            self.set_session(None);
        }
        Ok(dropped)
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
        self.publish_trust();
        let fresh = ok && self.trust.lock().is_ok_and(|t| t.fresh(now));
        self.fresh.store(fresh, std::sync::atomic::Ordering::SeqCst);
        dropped
    }

    async fn trust_round(&self) -> bool {
        let fetched = crate::web_updates::fetch_trust().await;
        self.apply_round(fetched, now() as u64)
    }

    /// Shows the statement in force to the native updater.
    fn publish_trust(&self) {
        if let Ok(trust) = self.trust.lock() {
            crate::web_trust::publish(trust.statement());
        }
    }

    fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().ok()?.clone()
    }

    fn set_session(&self, session: Option<Arc<Session>>) {
        if let Ok(mut current) = self.session.lock() {
            *current = session;
        }
    }

    /// The bundle new windows run: the selected instance's, else the newest.
    fn view(&self) -> Option<Held> {
        match self.session() {
            Some(session) => Some(session.held.clone()),
            None => self.store.lock().ok()?.newest().cloned(),
        }
    }

    fn jar_file(&self, origin: &Url) -> Option<std::path::PathBuf> {
        let name = hex::encode(&Sha256::digest(origin.as_str().as_bytes())[..8]);
        Some(
            self.data_dir
                .as_ref()?
                .join("sessions")
                .join(format!("{name}.json")),
        )
    }
}

/// Announces the verified bundle to the page before any of its scripts run.
/// The app honours it only on the host's own origin.
pub fn identity_script(app: &tauri::AppHandle, role: &str) -> String {
    let Some(held) = app.state::<Trusted>().view() else {
        return String::new();
    };
    let identity = serde_json::json!({
        "version": held.bundle.version.to_string(),
        "source": held.source.as_str(),
        "webClientApi": held.bundle.web_client_api,
        "keyId": held.bundle.key_id,
        "commit": held.bundle.commit,
        "role": role,
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

fn address(input: &str) -> Result<Url, String> {
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

async fn discover(origin: &Url) -> Result<ServerInfo, String> {
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
async fn choose(trusted: &Trusted, server: &ServerInfo) -> Result<Held, String> {
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

/// Answers one request of one window.
async fn handle(
    app: &tauri::AppHandle,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
) -> Resp {
    let trusted = app.state::<Trusted>();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let path_and_query = request
        .uri()
        .path_and_query()
        .map_or(path.clone(), |p| p.as_str().to_owned());
    let api = path == "/api" || path.starts_with("/api/");
    match label {
        "instance" | "attachment" => {
            let Some(session) = trusted.session() else {
                return plain(503, "No instance is selected.");
            };
            if api {
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
        "main" | "sidepeek" | "gaming-overlay" | "gaming-settings" => plain(404, "Not found."),
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

fn instance_window(window: &WebviewWindow) -> Result<Arc<Session>, String> {
    if window.label() != "instance" || !window.url().is_ok_and(|u| local_origin(&u).is_some()) {
        return Err("Only the instance window may use this.".into());
    }
    window
        .app_handle()
        .state::<Trusted>()
        .session()
        .ok_or_else(|| "No instance is selected.".into())
}

#[tauri::command]
fn desktop_ws_open(
    window: WebviewWindow,
    path: String,
    on_event: tauri::ipc::Channel<WsEvent>,
) -> Result<u64, String> {
    let session = instance_window(&window)?;
    if path != "/api/ws" {
        return Err("Only /api/ws can be opened.".into());
    }
    let trusted = window.app_handle().state::<Trusted>();
    crate::web_ws::open(
        &trusted.sockets,
        Arc::clone(&session.instance),
        move |event| on_event.send(event).is_ok(),
    )
}

#[tauri::command]
fn desktop_ws_send(window: WebviewWindow, id: u64, data: String) -> Result<(), String> {
    instance_window(&window)?;
    window
        .app_handle()
        .state::<Trusted>()
        .sockets
        .send(id, data)
}

#[tauri::command]
fn desktop_ws_close(window: WebviewWindow, id: u64) -> Result<(), String> {
    instance_window(&window)?;
    window.app_handle().state::<Trusted>().sockets.close(id);
    Ok(())
}

/// Window options every app window shares.
fn app_window<'a>(
    app: &'a tauri::AppHandle,
    label: &str,
    role: &str,
) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::CustomProtocol(app_url()))
        .use_https_scheme(true)
        .initialization_script(identity_script(app, role))
}

#[tauri::command]
async fn desktop_open_instance(
    window: WebviewWindow,
    state: tauri::State<'_, Trusted>,
    address: String,
) -> Result<(), String> {
    if window.label() != "main" || !window.url().is_ok_and(|u| bundled(&u)) {
        return Err("Only the instance selector may open an instance.".into());
    }
    let origin = self::address(&address)?;
    let server = discover(&origin).await?;
    let held = choose(&state, &server).await?;
    if window.app_handle().get_webview_window("instance").is_some() {
        return Err("Close the current instance before selecting another.".into());
    }
    let instance = Arc::new(Instance {
        client: client().map_err(|_| "Unable to initialize HTTPS.")?,
        jar: Jar::in_vault(
            state.jar_file(&origin),
            Arc::new(crate::web_vault::Os),
            now(),
        ),
        origin,
    });
    state.set_session(Some(Arc::new(Session { instance, held })));
    match open_instance_window(&window) {
        Ok(()) => Ok(()),
        Err(message) => {
            state.set_session(None);
            Err(message)
        }
    }
}

fn open_instance_window(window: &WebviewWindow) -> Result<(), String> {
    let app = window.app_handle().clone();
    let gaming_token = super::web_gaming::bind(&app);
    let popup_app = app.clone();
    let instance = app_window(&app, "instance", "instance")
        .title("Mnema Desktop DEV")
        .menu(super::web_gaming::menu(&app).map_err(|_| "Gaming settings unavailable.")?)
        // The bridge token exists only in a top-level page of the app origin
        // (a custom origin is "null" to `location.origin`, so compare the URL).
        .initialization_script(format!(
            "if(window===window.top&&window.location.href.startsWith({}))Object.defineProperty(window,'__MNEMA_GAMING_BRIDGE__',{{value:{}}});",
            serde_json::to_string(app_url().as_str()).map_err(|_| "Gaming setup failed.")?,
            serde_json::to_string(&gaming_token).map_err(|_| "Gaming setup failed.")?
        ))
        .inner_size(1200.0, 820.0)
        // Routes of the app, and the API paths that downloads and attachment
        // links navigate to. The proxy answers the latter.
        .on_navigation(move |url| bundled(url) || app_api(url))
        .on_new_window(move |url, features| {
            if app_api(&url) {
                if let Some(attachment) = popup_app.get_webview_window("attachment") {
                    let _ = attachment.navigate(url);
                    let _ = attachment.set_focus();
                } else {
                    // Reuse the opener's browser environment without granting
                    // native capabilities: the child gets no identity script.
                    let child = WebviewWindowBuilder::new(&popup_app, "attachment", WebviewUrl::CustomProtocol(url))
                        .use_https_scheme(true)
                        .window_features(features)
                        .title("Mnema Desktop DEV — Attachment")
                        .on_navigation(app_api)
                        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                        .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
                        .build();
                    if let Ok(window) = child {
                        return tauri::webview::NewWindowResponse::Create { window };
                    }
                }
            } else {
                open_external(&url);
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_permission_request(|_, _| tauri::webview::PermissionResponse::Default)
        .build()
        .map_err(|_| "Unable to open the instance. Check the WebView2 runtime.")?;
    let selector = window.clone();
    instance.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let trusted = selector.app_handle().state::<Trusted>();
            trusted.sockets.close_all();
            trusted.set_session(None);
            super::web_gaming::clear(selector.app_handle());
            if let Some(attachment) = selector.app_handle().get_webview_window("attachment") {
                let _ = attachment.close();
            }
            let _ = selector.show();
        }
    });
    window
        .hide()
        .map_err(|_| "Unable to hide the instance selector.")?;
    Ok(())
}

/// A path the proxy serves for the selected instance, on the app origin.
fn app_api(url: &Url) -> bool {
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
    // A window that was showing a withdrawn bundle, or none, moves to the
    // newly installed one, or shows why there is none.
    if (dropped || (!had_bundle && matches!(installed, Ok(Some(_)))))
        && let Some(main) = app.get_webview_window("main")
    {
        let _ = main.navigate(app_url());
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
            desktop_open_instance,
            desktop_ws_open,
            desktop_ws_send,
            desktop_ws_close,
            super::web_gaming::desktop_gaming_sync,
            super::web_gaming::desktop_gaming_snapshot,
            super::web_gaming::desktop_gaming_control,
            super::web_gaming::desktop_gaming_settings,
            super::native_updates::native_update_state,
            super::native_updates::native_update_action
        ])
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "gaming-settings" {
                let _ = super::web_gaming::open_settings(app);
            } else if event.id().as_ref() == super::native_updates::MENU_ID {
                super::native_updates::menu_clicked(app);
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
            super::native_updates::setup(app.handle());
            super::web_gaming::setup(app.handle());
            app_window(app.handle(), "main", "selector")
                .title("Mnema Desktop DEV — Instance")
                .menu(super::web_gaming::menu(app.handle())?)
                .inner_size(520.0, 520.0)
                .on_navigation(bundled)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_permission_request(|_, _| tauri::webview::PermissionResponse::Deny)
                .build()?;
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

fn open_external(url: &Url) {
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

    #[test]
    fn installs_need_a_fresh_statement_and_a_successful_fetch_in_this_round() {
        let doc = include_bytes!("../fixtures/trust.json").to_vec();
        let sig = include_bytes!("../fixtures/trust.json.sig").to_vec();
        let expiry = crate::web_trust::parse_utc("2027-04-15T21:28:34Z").unwrap();
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
        assert!(trusted.session().is_none());
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
