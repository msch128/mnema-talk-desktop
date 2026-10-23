//! The one window around the servers: a rail on the left, and the page of one
//! server (or the page for adding one) next to it.
//!
//! Every server has a webview of its own, at the host's app origin, with its
//! own browser storage (a WebView2 user-data folder on Windows, a data store
//! on macOS 14 and newer) and its own session in `instances`. A page learns
//! nothing of the others: the protocol handler and every command pick the
//! server by the label of the calling webview. The rail is a page of the host,
//! independent of any bundle and of any server; it is the only one that can
//! switch, reorder or remove a server. See `instances` for the bookkeeping.
use crate::instances::{
    AddError, Phase, RailState, Session, attachment_label, id_for, instance_label,
};
use crate::web_bundle::Held;
use crate::web_client::{Trusted, app_api, app_url, bundled, choose, discover};
use crate::web_proxy::{Instance, Jar, Limits, Resp, client, now};
use crate::web_vault::{Os, Vault};
use crate::web_ws::Sockets;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::{sync::Arc, time::Duration};
use tauri::{
    LogicalPosition, LogicalSize, Manager, Position, Rect, Size, Webview, WebviewUrl,
    WebviewWindowBuilder,
    menu::{Menu, MenuItem, Submenu},
    webview::WebviewBuilder,
    window::WindowBuilder,
};
use url::Url;

/// The rail's width in logical pixels.
pub const RAIL_WIDTH: f64 = 68.0;
const SHELL: &str = "shell";
const RAIL: &str = "rail";
/// The page for adding a server keeps the label of the old instance selector.
const ADD: &str = "main";
/// The Windows startup check looks for a window of this name.
const TITLE: &str = "Mnema Desktop DEV \u{2014} Instance";
#[cfg_attr(target_os = "macos", allow(dead_code))]
const PROFILES: &str = "profiles";
const WIPE_ATTEMPTS: u32 = 20;

const NEEDS_NEWER_MACOS: &str = "Several servers at once need macOS 14 or newer.";

// ----------------------------------------------------------------- rail page

const INDEX: &str = include_str!("../rail-ui/index.html");
const STYLE: &str = include_str!("../rail-ui/rail.css");
const SCRIPT: &str = include_str!("../rail-ui/rail.js");

/// What the rail page may do: load itself and talk to the host.
pub const RAIL_CSP: &str = concat!(
    "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; ",
    "connect-src ipc: http://ipc.localhost https://ipc.localhost; ",
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
);

/// The files of the rail page.
pub fn rail_asset(method: &str, path: &str) -> Resp {
    let (kind, body) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX),
        "/rail.css" => ("text/css; charset=utf-8", STYLE),
        "/rail.js" => ("text/javascript; charset=utf-8", SCRIPT),
        _ => return text(404, "Not found."),
    };
    if method != "GET" && method != "HEAD" {
        return text(405, "Method not allowed.");
    }
    Resp {
        status: 200,
        headers: vec![
            ("content-type".into(), kind.into()),
            ("content-security-policy".into(), RAIL_CSP.into()),
            ("x-content-type-options".into(), "nosniff".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body: if method == "HEAD" {
            Vec::new()
        } else {
            body.as_bytes().to_vec()
        },
    }
}

fn text(status: u16, body: &str) -> Resp {
    Resp {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("x-content-type-options".into(), "nosniff".into()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

// ------------------------------------------------------------------- storage

/// Whether the browser storage of two servers can be kept apart. WebView2
/// does it with a folder per server; WKWebView needs macOS 14 for a data
/// store of its own. Where it cannot, a second server is not started rather
/// than sharing storage (and the end-to-end keys in it) with the first.
fn storage_isolated() -> bool {
    #[cfg(target_os = "macos")]
    {
        static MAJOR: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
        MAJOR
            .get_or_init(|| {
                let out = std::process::Command::new("sw_vers")
                    .arg("-productVersion")
                    .output()
                    .ok()?;
                macos_major(&String::from_utf8_lossy(&out.stdout))
            })
            .is_some_and(|major| major >= 14)
    }
    #[cfg(not(target_os = "macos"))]
    true
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn macos_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// May one more server run next to those that do? The first always may.
fn room_for(running: usize) -> bool {
    running == 0 || storage_isolated()
}

#[cfg(not(target_os = "macos"))]
fn profile_dir(app: &tauri::AppHandle, id: &str) -> Option<PathBuf> {
    let dir = app.state::<Trusted>().data_dir()?.join(PROFILES).join(id);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(target_os = "macos")]
fn store_identifier(id: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("mnema-desktop-server:{id}").as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Where the host's own pages (the rail, the page for adding a server, the
/// gaming surfaces) keep their browser storage: apart from every server's, so
/// that the storage an old single-server client left can belong to one server.
const HOST_PROFILE: &str = "host";

#[cfg(target_os = "macos")]
fn host_identifier() -> [u8; 16] {
    store_identifier(HOST_PROFILE)
}

pub fn host_profile(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
) -> WebviewBuilder<tauri::Wry> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        builder.data_store_identifier(host_identifier())
    }
    #[cfg(not(target_os = "macos"))]
    match profile_dir(app, HOST_PROFILE) {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    }
}

/// The same for a window of the host.
pub fn host_profile_window<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle>,
    app: &tauri::AppHandle,
) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        builder.data_store_identifier(host_identifier())
    }
    #[cfg(not(target_os = "macos"))]
    match profile_dir(app, HOST_PROFILE) {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    }
}

/// Gives the page of a server browser storage of its own. The server that took
/// over the storage of the single-server client keeps using it (that is where
/// its end-to-end state is); no other page of the host touches it.
fn isolate(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
    id: &str,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let legacy = app
        .state::<Trusted>()
        .registry
        .lock()
        .is_ok_and(|r| r.legacy() == Some(id));
    if legacy {
        return Ok(builder);
    }
    #[cfg(target_os = "macos")]
    {
        Ok(builder.data_store_identifier(store_identifier(id)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let dir = profile_dir(app, id).ok_or("Unable to prepare storage for this server.")?;
        Ok(builder.data_directory(dir))
    }
}

/// Removes what a server's page stored in the browser engine. The webview is
/// already closed; the engine may hold its files a moment longer, so this
/// tries again for a while, in the background.
fn wipe_storage(app: &tauri::AppHandle, id: &str, legacy: bool) {
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        // The storage of the old client is the engine's default one. Its page
        // was cleared when it closed; on Windows the folder goes as well.
        #[cfg(not(target_os = "macos"))]
        if legacy && let Some(base) = app.state::<Trusted>().data_dir() {
            let default = base.join("EBWebView");
            for _ in 0..WIPE_ATTEMPTS {
                if !default.exists() || std::fs::remove_dir_all(&default).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        #[cfg(target_os = "macos")]
        let _ = legacy;
        #[cfg(target_os = "macos")]
        for _ in 0..WIPE_ATTEMPTS {
            if app.remove_data_store(store_identifier(&id)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let dir = app
                .state::<Trusted>()
                .data_dir()
                .map(|d| d.join(PROFILES).join(&id));
            for _ in 0..WIPE_ATTEMPTS {
                match &dir {
                    Some(dir) if dir.exists() && std::fs::remove_dir_all(dir).is_err() => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    _ => break,
                }
            }
        }
    });
}

/// Storage of servers that are no longer in the list (a removal that was cut
/// short by the end of the program) goes at the next start.
fn sweep_orphans(app: &tauri::AppHandle) {
    let known = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default();
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = app.state::<Trusted>().data_dir().map(|d| d.join(PROFILES))
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != HOST_PROFILE && !known.contains(&name) {
                wipe_storage(app, &name, false);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let keep: Vec<[u8; 16]> = known
                .iter()
                .map(|id| store_identifier(id))
                .chain(std::iter::once(host_identifier()))
                .collect();
            for stored in app.fetch_data_store_identifiers().await.unwrap_or_default() {
                if !keep.contains(&stored) {
                    let _ = app.remove_data_store(stored).await;
                }
            }
        });
    }
}

// -------------------------------------------------------------------- layout

fn content_labels(app: &tauri::AppHandle) -> Vec<String> {
    let ids = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default();
    std::iter::once(ADD.to_owned())
        .chain(ids.iter().map(|id| instance_label(id)))
        .collect()
}

/// The label of the page in front.
fn front_label(app: &tauri::AppHandle) -> String {
    let trusted = app.state::<Trusted>();
    let Ok(registry) = trusted.registry.lock() else {
        return ADD.to_owned();
    };
    match registry.active() {
        Some(id) if !registry.adding() => instance_label(id),
        _ => ADD.to_owned(),
    }
}

fn rect(x: f64, width: f64, height: f64) -> Rect {
    Rect {
        position: Position::Logical(LogicalPosition::new(x, 0.0)),
        size: Size::Logical(LogicalSize::new(width, height)),
    }
}

/// Puts the rail on the left, the page in front next to it and hides the
/// others. Hidden pages keep running (their sockets and counters stay alive),
/// the engine only stops painting them.
pub fn layout(app: &tauri::AppHandle) {
    let Some(window) = app.get_window(SHELL) else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (width, height) = (
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    );
    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.set_bounds(rect(0.0, RAIL_WIDTH, height));
    }
    let front = front_label(app);
    for label in content_labels(app) {
        let Some(page) = app.get_webview(&label) else {
            continue;
        };
        let _ = page.set_bounds(rect(RAIL_WIDTH, (width - RAIL_WIDTH).max(1.0), height));
        let _ = if label == front {
            page.show()
        } else {
            page.hide()
        };
    }
}

fn focus_front(app: &tauri::AppHandle) {
    if let Some(page) = app.get_webview(&front_label(app)) {
        let _ = page.set_focus();
    }
}

fn rail_snapshot(app: &tauri::AppHandle) -> RailState {
    let trusted = app.state::<Trusted>();
    let registry = trusted.registry.lock();
    match registry {
        Ok(registry) => registry.rail(room_for(registry.len())),
        Err(_) => RailState {
            active: None,
            adding: true,
            can_add: false,
            instances: Vec::new(),
        },
    }
}

/// Shows the rail what it should draw, and renames the menu entries to match.
pub fn push_rail(app: &tauri::AppHandle) {
    let state = rail_snapshot(app);
    if let Ok(json) = serde_json::to_string(&state)
        && let Some(rail) = app.get_webview(RAIL)
    {
        let _ = rail.eval(format!("window.mnemaRail&&window.mnemaRail.set({json})"));
    }
    if let Some(items) = app.try_state::<ServerMenu>() {
        items.update(&state);
    }
}

/// The counters changed: the icon badge is their sum, the rail shows each.
pub fn counts_changed(app: &tauri::AppHandle) {
    let total = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.badge())
        .unwrap_or(0);
    if let Some(window) = app.get_window(SHELL) {
        let _ = super::web_notify::apply_badge(&window, total);
    }
    push_rail(app);
}

fn changed(app: &tauri::AppHandle) {
    app.state::<Trusted>().save_instances();
    layout(app);
    push_rail(app);
}

/// Brings a server to the front (a toast was used, a gaming control was
/// pressed, the person chose it) and the window with it.
pub fn bring_forward(app: &tauri::AppHandle, id: &str) {
    let found = app
        .state::<Trusted>()
        .registry
        .lock()
        .is_ok_and(|mut r| r.activate(id));
    if !found {
        return;
    }
    changed(app);
    if let Some(window) = app.get_window(SHELL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    focus_front(app);
}

// ---------------------------------------------------------------------- menu

/// The entries of the Servers menu, so they can follow the list.
pub struct ServerMenu {
    items: Vec<MenuItem<tauri::Wry>>,
}

impl ServerMenu {
    fn update(&self, state: &RailState) {
        for (index, item) in self.items.iter().enumerate() {
            let server = state.instances.get(index);
            let _ = item.set_enabled(server.is_some());
            let _ = item.set_text(match server {
                Some(server) => format!("{} \u{2014} {}", index + 1, server.name),
                None => format!("Server {}", index + 1),
            });
        }
    }
}

/// Ctrl+1 to Ctrl+9 (Cmd on macOS) reach the servers; Ctrl+Shift+N adds one.
pub fn servers_menu(app: &tauri::AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let mut items = Vec::new();
    for number in 1..=crate::instances::MAX_INSTANCES {
        items.push(MenuItem::with_id(
            app,
            format!("server-{number}"),
            format!("Server {number}"),
            false,
            Some(format!("CmdOrCtrl+{number}")),
        )?);
    }
    let add = MenuItem::with_id(
        app,
        "server-add",
        "Add server\u{2026}",
        true,
        Some("CmdOrCtrl+Shift+N"),
    )?;
    let mut all: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    all.extend(
        items
            .iter()
            .map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>),
    );
    all.push(&add);
    let submenu = Submenu::with_items(app, "Servers", true, &all)?;
    app.manage(ServerMenu { items });
    Ok(submenu)
}

/// A menu entry was chosen.
pub fn menu_clicked(app: &tauri::AppHandle, id: &str) {
    if id == "server-add" {
        start_adding(app);
    } else if let Some(number) = id
        .strip_prefix("server-")
        .and_then(|n| n.parse::<usize>().ok())
    {
        let chosen = app
            .state::<Trusted>()
            .registry
            .lock()
            .ok()
            .and_then(|mut r| r.activate_nth(number));
        if chosen.is_some() {
            changed(app);
            focus_front(app);
        }
    }
}

// ------------------------------------------------------------------- windows

fn local_page(webview: &Webview, label: &str) -> bool {
    webview.label() == label && webview.url().is_ok_and(|u| bundled(&u))
}

fn host_page(app: &tauri::AppHandle, label: &str, role: &str) -> WebviewBuilder<tauri::Wry> {
    let builder = WebviewBuilder::new(label, WebviewUrl::CustomProtocol(app_url()))
        .use_https_scheme(true)
        .on_navigation(bundled)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_permission_request(crate::web_client::allow_permission);
    let builder = if role.is_empty() {
        builder
    } else {
        builder.initialization_script(crate::web_client::identity_script(app, role, None))
    };
    // The rail has no script of the probe: its policy would not let it report.
    let builder = if super::web_permissions::probe_enabled() && !role.is_empty() {
        builder.initialization_script(super::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    super::web_permissions::with_probe_args_child(app, label, host_profile(builder, app))
}

/// Opens the window with the rail and the page for adding a server, and starts
/// the servers of the list.
pub fn create(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = menu(app)?;
    let window = WindowBuilder::new(app, SHELL)
        .title(TITLE)
        .menu(menu)
        .inner_size(1200.0, 820.0)
        .min_inner_size(640.0, 480.0)
        .build()?;
    let (width, height) = {
        let size = window.inner_size()?;
        let scale = window.scale_factor()?;
        (
            f64::from(size.width) / scale,
            f64::from(size.height) / scale,
        )
    };
    window.add_child(
        host_page(app, RAIL, ""),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(RAIL_WIDTH, height),
    )?;
    window.add_child(
        host_page(app, ADD, "selector"),
        LogicalPosition::new(RAIL_WIDTH, 0.0),
        LogicalSize::new((width - RAIL_WIDTH).max(1.0), height),
    )?;
    if super::web_permissions::probe_enabled() {
        // A page labelled like the pages of servers, before any server exists,
        // so that the startup check can see how it is treated.
        let _ = window.add_child(
            host_page(app, "instance", "probe").focused(false),
            LogicalPosition::new(RAIL_WIDTH, 0.0),
            LogicalSize::new(1.0, 1.0),
        );
        super::web_gaming::open_probe_surfaces(app);
    }
    let events = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            layout(&events);
        }
        tauri::WindowEvent::Focused(true) => focus_front(&events),
        tauri::WindowEvent::Destroyed => shut_down(&events),
        _ => {}
    });
    sweep_orphans(app);
    let ids = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default();
    for id in ids {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { start_server(&app, &id).await });
    }
    layout(app);
    push_rail(app);
    #[cfg(feature = "multi-selftest")]
    crate::selftest_multi::start(app);
    Ok(())
}

fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let servers = servers_menu(app)?;
    let (gaming, update) = super::web_gaming::menu_items(app)?;
    #[cfg(target_os = "macos")]
    let menu = Menu::default(app)?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::new(app)?;
    menu.append(&servers)?;
    menu.append(&gaming)?;
    menu.append(&update)?;
    Ok(menu)
}

/// The window is gone: every connection ends and the program with it.
fn shut_down(app: &tauri::AppHandle) {
    if let Ok(registry) = app.state::<Trusted>().registry.lock() {
        for id in registry.ids() {
            if let Some(session) = registry.session(&id) {
                session.sockets.close_all();
            }
        }
    }
    super::web_gaming::clear(app);
    app.exit(0);
}

// ------------------------------------------------------------------- servers

fn make_session(
    trusted: &Trusted,
    id: &str,
    origin: Url,
    held: Held,
) -> Result<Arc<Session>, String> {
    Ok(Arc::new(Session {
        id: id.to_owned(),
        held,
        sockets: Arc::new(Sockets::default()),
        instance: Arc::new(Instance {
            client: client().map_err(|_| "Unable to initialize HTTPS.")?,
            jar: Jar::in_vault(trusted.jar_file(&origin), Arc::new(Os), now()),
            origin,
            limits: Limits::default(),
        }),
    }))
}

/// Checks a server and starts its page: the server is asked what it is, the
/// verified bundle for it is chosen (never its own JavaScript), and the page
/// opens. A server that cannot be served keeps its place in the list and says
/// why in its own tab.
async fn start_server(app: &tauri::AppHandle, id: &str) {
    let trusted = app.state::<Trusted>();
    let (origin, position) = {
        let Ok(mut registry) = trusted.registry.lock() else {
            return;
        };
        let position = registry.ids().iter().position(|i| i == id).unwrap_or(0);
        let Some(slot) = registry.slot_mut(id) else {
            return;
        };
        slot.phase = Phase::Starting;
        (slot.origin.clone(), position)
    };
    push_rail(app);
    let outcome = if room_for(position) {
        connect(&trusted, id, &origin).await
    } else {
        Err(NEEDS_NEWER_MACOS.to_owned())
    };
    let ready = match outcome {
        Ok(session) => {
            let set = trusted
                .registry
                .lock()
                .ok()
                .and_then(|mut r| {
                    r.slot_mut(id).map(|slot| {
                        slot.restart();
                        slot.phase = Phase::Ready(Arc::clone(&session));
                    })
                })
                .is_some();
            set.then_some(session)
        }
        Err(why) => {
            if let Ok(mut registry) = trusted.registry.lock()
                && let Some(slot) = registry.slot_mut(id)
            {
                slot.phase = Phase::Refused(why);
            }
            None
        }
    };
    if let Some(session) = ready
        && let Err(why) = open_page(app, id, &session).await
        && let Ok(mut registry) = trusted.registry.lock()
        && let Some(slot) = registry.slot_mut(id)
    {
        slot.phase = Phase::Refused(why);
    }
    refresh_pages(app, id);
    push_rail(app);
}

async fn connect(trusted: &Trusted, id: &str, origin: &Url) -> Result<Arc<Session>, String> {
    let server = discover(origin).await?;
    let held = choose(trusted, &server).await?;
    make_session(trusted, id, origin.clone(), held)
}

/// A page that already exists reloads, to show what its server's slot says
/// now.
fn refresh_pages(app: &tauri::AppHandle, id: &str) {
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.navigate(app_url());
    }
    layout(app);
}

/// The bundle set changed (a key was revoked, a cap withdrew one, the first
/// bundle arrived): selector and pages move to what is held now, and servers
/// that waited for a bundle start.
pub fn bundles_changed(app: &tauri::AppHandle) {
    if let Some(selector) = app.get_webview(ADD) {
        let _ = selector.navigate(app_url());
    }
    let unready: Vec<String> = app
        .state::<Trusted>()
        .registry
        .lock()
        .map(|r| {
            r.ids()
                .into_iter()
                .filter(|id| r.session(id).is_none())
                .collect()
        })
        .unwrap_or_default();
    for id in unready {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { start_server(&app, &id).await });
    }
    push_rail(app);
}

/// Creates the webview of a running server.
async fn open_page(app: &tauri::AppHandle, id: &str, session: &Arc<Session>) -> Result<(), String> {
    if app.get_webview(&instance_label(id)).is_some() {
        return Ok(());
    }
    let window = app.get_window(SHELL).ok_or("The window is gone.")?;
    let builder = page_builder(app, id, session)?;
    let page = window
        .add_child(
            builder,
            LogicalPosition::new(RAIL_WIDTH, 0.0),
            LogicalSize::new(1.0, 1.0),
        )
        .map_err(|_| "Unable to open the server. Check the WebView2 runtime.")?;
    let _ = page.hide();
    layout(app);
    Ok(())
}

fn page_builder(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<Session>,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let label = instance_label(id);
    let gaming_token = super::web_gaming::bind(app, id);
    let popup_app = app.clone();
    let opener = id.to_owned();
    let builder = WebviewBuilder::new(&label, WebviewUrl::CustomProtocol(app_url()))
        .use_https_scheme(true)
        .initialization_script(crate::web_client::identity_script(
            app,
            "instance",
            Some(&session.held),
        ))
        // The bridge token exists only in a top-level page of the app origin
        // (a custom origin is "null" to `location.origin`, so compare the URL).
        .initialization_script(format!(
            "if(window===window.top&&window.location.href.startsWith({}))Object.defineProperty(window,'__MNEMA_GAMING_BRIDGE__',{{value:{}}});",
            serde_json::to_string(app_url().as_str()).map_err(|_| "Gaming setup failed.")?,
            serde_json::to_string(&gaming_token).map_err(|_| "Gaming setup failed.")?
        ))
        // Routes of the app, and the API paths that downloads and attachment
        // links navigate to. The proxy answers the latter.
        .on_navigation(move |url| bundled(url) || app_api(url))
        .on_new_window(move |url, features| new_window(&popup_app, &opener, url, features))
        .on_permission_request(crate::web_client::allow_permission);
    let builder = if super::web_permissions::probe_enabled() {
        builder.initialization_script(super::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    isolate(builder, app, id)
}

/// A link of a server's page: attachments and downloads open in a window of
/// their own (tied to that server), everything else goes to the browser after
/// a confirmation.
fn new_window(
    app: &tauri::AppHandle,
    id: &str,
    url: Url,
    features: tauri::webview::NewWindowFeatures,
) -> tauri::webview::NewWindowResponse<tauri::Wry> {
    use tauri::webview::NewWindowResponse::{Create, Deny};
    if !app_api(&url) {
        crate::web_client::open_external(&url);
        return Deny;
    }
    let label = attachment_label(id);
    if let Some(attachment) = app.get_webview_window(&label) {
        let _ = attachment.navigate(url);
        let _ = attachment.set_focus();
        return Deny;
    }
    // Reuse the opener's browser environment without granting native
    // capabilities: the child gets no identity script.
    let child = super::web_permissions::with_probe_args(
        app,
        &label,
        WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(url))
            .use_https_scheme(true)
            .window_features(features)
            .title("Mnema Desktop DEV \u{2014} Attachment")
            .on_navigation(app_api)
            .on_new_window(|_, _| Deny)
            .on_permission_request(crate::web_client::allow_permission),
    )
    .build();
    match child {
        Ok(window) => Create { window },
        Err(_) => Deny,
    }
}

// ------------------------------------------------------------------ commands

/// The page for adding a server asks to open one.
#[tauri::command]
pub async fn desktop_open_instance(
    webview: Webview,
    state: tauri::State<'_, Trusted>,
    address: String,
) -> Result<(), String> {
    if !local_page(&webview, ADD) {
        return Err("Only the instance selector may open an instance.".into());
    }
    add_server(webview.app_handle(), &state, &address).await
}

/// Adds a server: the address is checked, the server asked what it is, the
/// verified bundle for it chosen, and only then does it join the list, so a
/// server that cannot be served never takes a place.
pub async fn add_server(
    app: &tauri::AppHandle,
    state: &Trusted,
    address: &str,
) -> Result<(), String> {
    let app = app.clone();
    let origin = crate::web_client::address(address)?;
    let id = id_for(&origin);
    let known = state.registry.lock().map_err(|_| "busy")?.contains(&id);
    if known {
        bring_forward(&app, &id);
        return Ok(());
    }
    check_room(state)?;
    let server = discover(&origin).await?;
    let held = choose(state, &server).await?;
    let adopts = state.adopts_old_storage(&origin, &Os);
    let session = make_session(state, &id, origin.clone(), held)?;
    {
        let mut registry = state.registry.lock().map_err(|_| "busy")?;
        match registry.add(origin) {
            Ok(_) | Err(AddError::Known(_)) => {}
            Err(full) => return Err(full.to_string()),
        }
        if let Some(slot) = registry.slot_mut(&id) {
            slot.restart();
            slot.phase = Phase::Ready(Arc::clone(&session));
        }
        if adopts {
            registry.set_legacy(&id);
        }
        registry.activate(&id);
    }
    if let Err(why) = open_page(&app, &id, &session).await {
        remove_server(&app, &id).await;
        return Err(why);
    }
    changed(&app);
    focus_front(&app);
    Ok(())
}

fn check_room(state: &Trusted) -> Result<(), String> {
    let registry = state.registry.lock().map_err(|_| "busy")?;
    if registry.len() >= crate::instances::MAX_INSTANCES {
        return Err(AddError::Full.to_string());
    }
    if !room_for(registry.len()) {
        return Err(NEEDS_NEWER_MACOS.into());
    }
    Ok(())
}

/// Only the rail may drive the list.
fn from_rail(webview: &Webview) -> Result<tauri::AppHandle, String> {
    if local_page(webview, RAIL) {
        Ok(webview.app_handle().clone())
    } else {
        Err("Only the server rail may use this.".into())
    }
}

#[tauri::command]
pub fn rail_state(webview: Webview) -> Result<RailState, String> {
    Ok(rail_snapshot(&from_rail(&webview)?))
}

#[tauri::command]
pub fn rail_activate(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    let found = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .activate(&id);
    if found {
        changed(&app);
        focus_front(&app);
    }
    Ok(())
}

fn start_adding(app: &tauri::AppHandle) {
    let started = app
        .state::<Trusted>()
        .registry
        .lock()
        .is_ok_and(|mut r| room_for(r.len()) && r.start_adding());
    if started {
        changed(app);
        focus_front(app);
    }
}

#[tauri::command]
pub fn rail_add(webview: Webview) -> Result<(), String> {
    start_adding(&from_rail(&webview)?);
    Ok(())
}

#[tauri::command]
pub fn rail_cancel_add(webview: Webview) -> Result<(), String> {
    let app = from_rail(&webview)?;
    app.state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .stop_adding();
    changed(&app);
    focus_front(&app);
    Ok(())
}

#[tauri::command]
pub fn rail_move(webview: Webview, id: String, to: u32) -> Result<(), String> {
    let app = from_rail(&webview)?;
    app.state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .move_to(&id, to as usize);
    changed(&app);
    Ok(())
}

#[tauri::command]
pub async fn rail_remove(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    remove_server(&app, &id).await;
    Ok(())
}

#[tauri::command]
pub async fn rail_reconnect(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    let refused = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id)
        .is_none();
    if refused {
        start_server(&app, &id).await;
    }
    Ok(())
}

/// Takes a server out of the list and wipes everything this device holds of
/// it: its stored session (keychain entry and fallback file), its browser
/// storage, its sockets, its toasts and its gaming token.
pub(crate) async fn remove_server(app: &tauri::AppHandle, id: &str) {
    let trusted = app.state::<Trusted>();
    let (slot, legacy_before) = match trusted.registry.lock() {
        Ok(mut registry) => {
            let legacy = registry.legacy().map(str::to_owned);
            (registry.remove(id), legacy)
        }
        Err(_) => return,
    };
    let Some(slot) = slot else {
        return;
    };
    let legacy = legacy_before.as_deref() == Some(id);
    match &slot.phase {
        Phase::Ready(session) => {
            session.sockets.close_all();
            session.instance.jar.forget();
        }
        _ => forget_stored_session(&trusted, &slot.origin, id),
    }
    super::web_gaming::forget(app, id);
    super::web_notify::closed(app, id);
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.clear_all_browsing_data();
        let _ = page.close();
    }
    if let Some(attachment) = app.get_webview_window(&attachment_label(id)) {
        let _ = attachment.close();
    }
    wipe_storage(app, id, legacy);
    changed(app);
    counts_changed(app);
    focus_front(app);
}

/// The stored session of a server that never started in this run.
fn forget_stored_session(trusted: &Trusted, origin: &Url, id: &str) {
    Os.remove(id);
    if let Some(file) = trusted.jar_file(origin) {
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_page_is_served_with_its_own_strict_policy_and_nothing_else() {
        for path in ["/", "/index.html", "/rail.css", "/rail.js"] {
            let resp = rail_asset("GET", path);
            assert_eq!(resp.status, 200, "{path}");
            let csp = resp
                .headers
                .iter()
                .find(|(n, _)| n == "content-security-policy")
                .map(|(_, v)| v.as_str());
            assert_eq!(csp, Some(RAIL_CSP));
            assert!(!resp.body.is_empty());
        }
        assert!(!RAIL_CSP.contains("unsafe-inline") && !RAIL_CSP.contains("unsafe-eval"));
        assert!(RAIL_CSP.contains("default-src 'none'"));
        assert_eq!(rail_asset("HEAD", "/rail.js").body.len(), 0);
        assert_eq!(rail_asset("POST", "/rail.js").status, 405);
        for path in [
            "/api/health",
            "/c/1",
            "/rail.js/",
            "/index.html?x",
            "/../x",
            "",
        ] {
            assert_eq!(rail_asset("GET", path).status, 404, "{path}");
        }
    }

    #[test]
    fn the_page_may_only_be_a_page_of_the_host() {
        // The rail and the adding page are the only host pages with commands;
        // their labels are not labels of a server's page.
        for label in [RAIL, ADD, SHELL] {
            assert!(crate::instances::id_of_instance_label(label).is_none());
        }
    }

    #[test]
    fn macos_versions_are_read_from_the_system() {
        assert_eq!(macos_major("14.6.1\n"), Some(14));
        assert_eq!(macos_major("26.0"), Some(26));
        assert_eq!(macos_major("12"), Some(12));
        assert_eq!(macos_major(""), None);
        assert_eq!(macos_major("x.1"), None);
    }

    #[test]
    fn the_first_server_always_runs_and_more_need_separate_storage() {
        assert!(room_for(0));
        assert_eq!(room_for(1), storage_isolated());
    }

    #[test]
    fn the_window_keeps_the_title_the_startup_check_looks_for() {
        assert_eq!(TITLE, "Mnema Desktop DEV \u{2014} Instance");
        const { assert!(RAIL_WIDTH > 40.0 && RAIL_WIDTH < 120.0) };
    }

    /// Who may call what is decided by the capability files: the rail drives
    /// the list and nothing else; a server's page reaches its own socket,
    /// toasts, counters and gaming bridge and can neither drive the list nor
    /// add a server; only the page for adding may do that.
    #[test]
    fn the_commands_are_split_between_the_rail_the_pages_and_the_selector() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        let mut rail_seen = false;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let id = json["identifier"].as_str().unwrap().to_owned();
            let permissions: Vec<&str> = json["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_str())
                .collect();
            let names = |prefix: &str| permissions.iter().any(|p| p.starts_with(prefix));
            if id == "server-rail" {
                rail_seen = true;
                assert_eq!(json["webviews"], serde_json::json!(["rail"]));
                assert!(json.get("windows").is_none());
                assert!(permissions.iter().all(|p| p.starts_with("allow-rail-")));
                assert_eq!(permissions.len(), 7);
            } else {
                assert!(!names("allow-rail-"), "{id} may drive the list");
            }
            if json["webviews"] == serde_json::json!(["i-*"]) {
                assert!(
                    !names("allow-desktop-open-instance"),
                    "{id}: a page of a server may not add servers"
                );
                assert!(
                    json.get("windows").is_none(),
                    "{id}: by webview, not window"
                );
            }
        }
        assert!(rail_seen);
        // The pages of servers, the rail and the selector are different
        // webviews, so no capability can reach two of them by accident.
        assert_ne!(RAIL, ADD);
    }

    #[test]
    fn the_pages_of_the_host_do_not_share_a_label() {
        assert_ne!(ADD, RAIL);
        assert_ne!(ADD, SHELL);
        assert_ne!(RAIL, SHELL);
    }
}
