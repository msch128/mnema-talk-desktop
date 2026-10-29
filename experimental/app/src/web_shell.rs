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
use crate::web_proxy::{Instance, Jar, Limits, Resp, client, forward, now};
use crate::web_vault::{Os, Vault};
use crate::web_ws::Sockets;
#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
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
const PRODUCT: &str = "Mnema Desktop DEV";
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
        data_stores_available(*MAJOR.get_or_init(|| {
            let out = std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()?;
            macos_major(&String::from_utf8_lossy(&out.stdout))
        }))
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// WKWebView's data store API (creating, listing and removing stores) exists
/// from macOS 14. An unknown version is treated as too old.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn data_stores_available(major: Option<u32>) -> bool {
    major.is_some_and(|major| major >= 14)
}

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn macos_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// May one more server run next to those that do? The first always may.
fn room_for(running: usize) -> bool {
    room_with(running, storage_isolated())
}

fn room_with(running: usize, isolated: bool) -> bool {
    running == 0 || isolated
}

#[cfg(not(target_os = "macos"))]
fn profile_dir(app: &tauri::AppHandle, id: &str) -> Result<PathBuf, String> {
    let trusted = app.state::<Trusted>();
    let base = trusted
        .data_dir()
        .ok_or("This client has no data directory for the storage of a server.")?;
    let dir = base.join(PROFILES).join(id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Unable to prepare storage for this server ({e})."))?;
    Ok(dir)
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

/// Puts a builder of a host page on the host's storage: the same policy for a
/// webview and for a window, whose builders share no trait. Where the folder
/// cannot be prepared the page falls back to the engine's default storage,
/// which is said on stderr rather than left silent.
macro_rules! on_host_storage {
    ($builder:expr, $app:expr) => {{
        #[cfg(target_os = "macos")]
        {
            let _ = $app;
            $builder.data_store_identifier(host_identifier())
        }
        #[cfg(not(target_os = "macos"))]
        {
            match profile_dir($app, HOST_PROFILE) {
                Ok(dir) => $builder.data_directory(dir),
                Err(why) => {
                    eprintln!("Mnema Desktop: host pages use the default storage: {why}");
                    $builder
                }
            }
        }
    }};
}

pub fn host_profile(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
) -> WebviewBuilder<tauri::Wry> {
    on_host_storage!(builder, app)
}

/// The same for a window of the host.
pub fn host_profile_window<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle>,
    app: &tauri::AppHandle,
) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle> {
    on_host_storage!(builder, app)
}

/// Gives the page of a server browser storage of its own. Every server gets
/// one; the storage of the single-server client is never handed to any.
fn isolate(
    builder: WebviewBuilder<tauri::Wry>,
    app: &tauri::AppHandle,
    id: &str,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let _ = app;
    #[cfg(target_os = "macos")]
    {
        Ok(builder.data_store_identifier(store_identifier(id)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(builder.data_directory(profile_dir(app, id)?))
    }
}

/// Removes what a server's page stored in the browser engine. The webview is
/// already closed; the engine may hold its files a moment longer, so this
/// tries again for a while, in the background. A server that joins the list
/// again meanwhile (same address, same storage) ends the wipe.
fn wipe_storage(app: &tauri::AppHandle, id: &str) {
    #[cfg(target_os = "macos")]
    if !storage_isolated() {
        // Below macOS 14 there is no store of the server's own to remove.
        return;
    }
    let app = app.clone();
    let id = id.to_owned();
    tauri::async_runtime::spawn(async move {
        #[cfg(target_os = "macos")]
        for _ in 0..WIPE_ATTEMPTS {
            if readded(&app, &id) {
                break;
            }
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
                if readded(&app, &id) {
                    break;
                }
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

/// Whether the server is on the list again.
fn readded(app: &tauri::AppHandle, id: &str) -> bool {
    registry_ids(app).iter().any(|i| i == id)
}

/// The ids of the list, in order.
fn registry_ids(app: &tauri::AppHandle) -> Vec<String> {
    app.state::<Trusted>()
        .registry
        .lock()
        .map(|r| r.ids())
        .unwrap_or_default()
}

/// Starts a server in the background.
fn spawn_start(app: &tauri::AppHandle, id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { start_server(&app, &id).await });
}

/// Wipes the engine's default storage: where the single-server client kept
/// everything (cookies, IndexedDB with the end-to-end state, caches). It works
/// with no page open: a hidden webview on the default storage is made for the
/// purpose, asked to remove every kind of website data, and closed. On
/// Windows the engine's folder goes too.
pub(crate) async fn wipe_default_storage(app: &tauri::AppHandle) {
    if let Ok(page) = open_default_page(app, "wipe").await {
        let _ = page.clear_all_browsing_data();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = page.close();
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(base) = app.state::<Trusted>().data_dir() {
        let default = base.join("EBWebView");
        for _ in 0..WIPE_ATTEMPTS {
            if !default.exists() || std::fs::remove_dir_all(&default).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// A hidden webview on the engine's default storage (never a server's, never
/// the host's own pages').
pub(crate) async fn open_default_page(
    app: &tauri::AppHandle,
    label: &str,
) -> Result<Webview, String> {
    let window = app.get_window(SHELL).ok_or("The window is gone.")?;
    let page = window
        .add_child(
            WebviewBuilder::new(label, WebviewUrl::CustomProtocol(app_url()))
                .use_https_scheme(true)
                .on_navigation(bundled),
            LogicalPosition::new(0.0, 0.0),
            LogicalSize::new(1.0, 1.0),
        )
        .map_err(|e| format!("Unable to reach the old storage ({e})."))?;
    let _ = page.hide();
    Ok(page)
}

/// Storage of servers that are no longer in the list (a removal that was cut
/// short by the end of the program) goes at the next start.
fn sweep_orphans(app: &tauri::AppHandle) {
    // An empty list that is only the result of an unusable file proves nothing
    // about which storage is orphaned; the next start with a good list sweeps.
    if app.state::<Trusted>().list_unreadable() {
        return;
    }
    let known = registry_ids(app);
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = app.state::<Trusted>().data_dir().map(|d| d.join(PROFILES))
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != HOST_PROFILE && !known.contains(&name) {
                wipe_storage(app, &name);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if storage_isolated() {
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

/// How long one hand-off of focus owns the window.
///
/// On Windows the hand-off posts `MoveFocus`, and the webview answers by
/// taking focus. That answer is another focus report. Handling it with a
/// second hand-off is the caret blinking as fast as the message loop.
const FOCUS_HOLD: Duration = Duration::from_millis(300);

/// Size, scale, the page in front, and which pages exist.
///
/// Placing again when none of these changed calls `show`. On Windows `show`
/// activates a webview that is already visible.
#[derive(Clone, PartialEq)]
struct Frame {
    width: u32,
    height: u32,
    scale_bits: u64,
    front: String,
    pages: Vec<String>,
}

#[derive(Default)]
struct ShellPlacement {
    frame: Option<Frame>,
    handed: Option<Instant>,
}

/// What a layout should do with the page in front.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    /// Nothing moved. `show` would only activate the page again.
    Same,
    /// The front page is new, or it just became the front page.
    ShowFront,
    /// The front page is already up. Move bounds, and do not activate it.
    KeepFront,
}

/// Whether this instant may hand focus to the page. A repeat inside the hold
/// is the echo of the hand-off that just ran, including the deactivation
/// WebView2 posts between the two reports, so losing focus does not re-arm it.
fn handoff_due(handed: &mut Option<Instant>, now: Instant) -> bool {
    let held = handed.is_some_and(|at| now.saturating_duration_since(at) < FOCUS_HOLD);
    if held {
        return false;
    }
    *handed = Some(now);
    true
}

fn place(prev: &mut Option<Frame>, next: &Frame) -> Place {
    let decision = match prev.as_ref() {
        Some(current) if current == next => Place::Same,
        Some(current)
            if current.front == next.front
                && current.pages.iter().any(|label| label == &next.front) =>
        {
            Place::KeepFront
        }
        _ => Place::ShowFront,
    };
    if decision != Place::Same {
        *prev = Some(next.clone());
    }
    decision
}

fn placement(app: &tauri::AppHandle) -> Option<tauri::State<'_, Mutex<ShellPlacement>>> {
    app.try_state::<Mutex<ShellPlacement>>()
}

/// Labels of the pages that already have a webview, in list order.
fn present_pages(app: &tauri::AppHandle) -> Vec<String> {
    content_labels(app)
        .into_iter()
        .filter(|label| app.get_webview(label).is_some())
        .collect()
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
    let frame = Frame {
        width: size.width,
        height: size.height,
        scale_bits: scale.to_bits(),
        front: front_label(app),
        pages: present_pages(app),
    };
    let place = take_frame(app, &frame);
    if place == Place::Same {
        return;
    }
    let (width, height) = (
        f64::from(frame.width) / scale,
        f64::from(frame.height) / scale,
    );
    if let Some(rail) = app.get_webview(RAIL) {
        let _ = rail.set_bounds(rect(0.0, RAIL_WIDTH, height));
    }
    for label in content_labels(app) {
        let Some(page) = app.get_webview(&label) else {
            continue;
        };
        let _ = page.set_bounds(rect(RAIL_WIDTH, (width - RAIL_WIDTH).max(1.0), height));
        if label == frame.front {
            if place == Place::ShowFront {
                let _ = page.show();
            }
        } else {
            let _ = page.hide();
        }
    }
}

fn take_frame(app: &tauri::AppHandle, frame: &Frame) -> Place {
    let Some(state) = placement(app) else {
        return Place::ShowFront;
    };
    let Ok(mut slot) = state.lock() else {
        return Place::ShowFront;
    };
    place(&mut slot.frame, frame)
}

/// Moves focus to the front page and starts the hold, so the focus report
/// that follows does not move it again.
fn focus_front(app: &tauri::AppHandle) {
    if let Some(state) = placement(app)
        && let Ok(mut slot) = state.lock()
    {
        slot.handed = Some(Instant::now());
    }
    move_focus(app);
}

fn move_focus(app: &tauri::AppHandle) {
    if let Some(page) = app.get_webview(&front_label(app)) {
        let _ = page.set_focus();
    }
}

/// The window itself gained focus. One hand-off per hold; the echo is ignored.
fn on_window_focused(app: &tauri::AppHandle) {
    let due = match placement(app) {
        Some(state) => state
            .lock()
            .map(|mut slot| handoff_due(&mut slot.handed, Instant::now()))
            .unwrap_or(true),
        None => true,
    };
    if due {
        move_focus(app);
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
/// The title of the window: the active server's name before the product, or
/// the product alone while none is active.
fn window_title(state: &RailState) -> String {
    state
        .instances
        .iter()
        .find(|i| state.active.as_deref() == Some(i.id.as_str()))
        .map_or_else(
            || TITLE.to_owned(),
            |i| format!("{} \u{2014} {PRODUCT}", i.name),
        )
}

pub fn push_rail(app: &tauri::AppHandle) {
    let state = rail_snapshot(app);
    // The window is titled with the server in front, by the name it gives itself.
    if let Some(window) = app.get_window(SHELL) {
        let _ = window.set_title(&window_title(&state));
    }
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
    app.manage(Mutex::new(ShellPlacement::default()));
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
        tauri::WindowEvent::Focused(true) => on_window_focused(&events),
        // macOS keeps the program running behind a closed window (a call, the
        // Dock badge and the toasts go on); the Dock icon brings it back.
        #[cfg(target_os = "macos")]
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            if let Some(shell) = events.get_window(SHELL) {
                let _ = shell.hide();
            }
        }
        tauri::WindowEvent::Destroyed => shut_down(&events),
        _ => {}
    });
    // The storage of the single-server client belongs to no one server: wipe
    // it once, then write the list, which ends the wipe for every later start.
    if app.state::<Trusted>().take_old_storage_wipe() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            wipe_default_storage(&app).await;
            app.state::<Trusted>().save_instances();
        });
    }
    sweep_orphans(app);
    for id in registry_ids(app) {
        spawn_start(app, id);
    }
    layout(app);
    push_rail(app);
    #[cfg(feature = "multi-selftest")]
    crate::selftest_multi::start(app);
    Ok(())
}

/// The Dock icon was clicked while the window is hidden.
#[cfg(target_os = "macos")]
pub fn reopen(app: &tauri::AppHandle) {
    if let Some(shell) = app.get_window(SHELL) {
        let _ = shell.show();
        let _ = shell.set_focus();
    }
}

fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let servers = servers_menu(app)?;
    let (gaming, update) = super::web_gaming::menu_items(app)?;
    #[cfg(target_os = "macos")]
    let menu = Menu::default(app)?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::new(app)?;
    menu.append(&servers)?;
    // A main menu on macOS holds submenus only: the updater goes into the
    // application menu, and the gaming overlay does not exist there.
    #[cfg(target_os = "macos")]
    {
        let _ = gaming;
        if let Some(application) = menu.items()?.iter().find_map(|i| i.as_submenu()) {
            application.append(&update)?;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        menu.append(&gaming)?;
        menu.append(&update)?;
    }
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
    community: Option<String>,
) -> Result<Arc<Session>, String> {
    Ok(Arc::new(Session {
        id: id.to_owned(),
        held,
        community,
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
    // One start at a time per server: a second caller (the bundle arriving, a
    // click on Reconnect) finds the first one running and leaves it be.
    let (origin, position) = {
        let Ok(mut registry) = trusted.registry.lock() else {
            return;
        };
        let Some(claim) = registry.claim_start(id) else {
            return;
        };
        claim
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
                        slot.settle(Phase::Ready(Arc::clone(&session)));
                    })
                })
                .is_some();
            // A key revoked while this server started: the session is ended
            // and the tab says why, instead of running the withdrawn bundle.
            (set && trusted.keep_if_held(id, &session)).then_some(session)
        }
        Err(why) => {
            if let Ok(mut registry) = trusted.registry.lock()
                && let Some(slot) = registry.slot_mut(id)
            {
                slot.settle(Phase::Refused(why));
            }
            None
        }
    };
    if let Some(session) = ready
        && let Err(why) = open_page(app, id, &session, None).await
        && let Ok(mut registry) = trusted.registry.lock()
        && let Some(slot) = registry.slot_mut(id)
    {
        slot.settle(Phase::Refused(why));
    }
    refresh_pages(app, id);
    push_rail(app);
}

async fn connect(trusted: &Trusted, id: &str, origin: &Url) -> Result<Arc<Session>, String> {
    let (server, community) = discover(origin).await?;
    let held = choose(trusted, &server).await?;
    make_session(trusted, id, origin.clone(), held, community)
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
        spawn_start(app, id);
    }
    push_rail(app);
}

/// Creates the webview of a running server.
async fn open_page(
    app: &tauri::AppHandle,
    id: &str,
    session: &Arc<Session>,
    invite: Option<&str>,
) -> Result<(), String> {
    if app.get_webview(&instance_label(id)).is_some() {
        return Ok(());
    }
    let window = app.get_window(SHELL).ok_or("The window is gone.")?;
    let builder = page_builder(app, id, session, invite)?;
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
    invite: Option<&str>,
) -> Result<WebviewBuilder<tauri::Wry>, String> {
    let label = instance_label(id);
    let gaming_token = super::web_gaming::bind(app, id);
    let popup_app = app.clone();
    let opener = id.to_owned();
    let sockets = Arc::clone(&session.sockets);
    let builder = WebviewBuilder::new(
        &label,
        WebviewUrl::CustomProtocol(crate::web_client::join_page(invite)),
    )
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
        // A reload or navigation ends the document that opened the host-side
        // sockets; nothing else would close them. Only app routes count: a
        // download navigation keeps the document, and its sockets, alive. A
        // page that loses a socket reconnects on its own.
        .on_page_load(move |_, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Started)
                && bundled(payload.url())
            {
                sockets.document_replaced();
            }
        })
        .on_new_window(move |url, features| new_window(&popup_app, &opener, url, features))
        .on_permission_request(crate::web_client::allow_permission);
    let builder = if super::web_permissions::probe_enabled() {
        builder.initialization_script(super::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    // WKWebView saves nothing unless the app takes the download; WebView2
    // has a download UI of its own.
    #[cfg(target_os = "macos")]
    let builder = {
        let downloads = app.clone();
        builder.on_download(move |_, event| save_download(&downloads, event))
    };
    isolate(builder, app, id)
}

/// Takes a download of the app's own API into the Downloads folder, under a
/// name that cannot leave it and does not overwrite anything.
#[cfg(target_os = "macos")]
fn save_download(app: &tauri::AppHandle, event: tauri::webview::DownloadEvent<'_>) -> bool {
    use tauri::webview::DownloadEvent::{Finished, Requested};
    match event {
        Requested { url, destination } => {
            let Some(dir) = app.path().download_dir().ok().filter(|_| app_api(&url)) else {
                return false;
            };
            let suggested = destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            *destination = download_target(&dir, &suggested, |p| p.exists());
            true
        }
        Finished { .. } => true,
        _ => false,
    }
}

/// Where a download goes in `dir`: the suggested name reduced to a plain file
/// name, with " (2)", " (3)" and so on before the extension while `taken`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn download_target(
    dir: &std::path::Path,
    suggested: &str,
    taken: impl Fn(&std::path::Path) -> bool,
) -> std::path::PathBuf {
    let name: String = suggested
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let name = name.trim().trim_start_matches('.');
    let name = if name.is_empty() { "download" } else { name };
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .map_or_else(|| name.to_owned(), |s| s.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut target = dir.join(name);
    let mut n = 2;
    while taken(&target) {
        target = dir.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    target
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
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(url))
        .use_https_scheme(true)
        .window_features(features)
        .title("Mnema Desktop DEV \u{2014} Attachment")
        .on_navigation(app_api)
        .on_new_window(|_, _| Deny)
        .on_permission_request(crate::web_client::allow_permission);
    #[cfg(target_os = "macos")]
    let builder = {
        let downloads = app.clone();
        builder.on_download(move |_, event| save_download(&downloads, event))
    };
    let child = super::web_permissions::with_probe_args(app, &label, builder).build();
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
    let invite = crate::web_client::invite_code(address);
    let id = id_for(&origin);
    let known = state.registry.lock().map_err(|_| "busy")?.contains(&id);
    if known {
        // A second paste of the invite opens the join screen only while the
        // page is still the door. A server already in use stays where it is.
        offer_invite(&app, &id, invite.as_deref());
        bring_forward(&app, &id);
        return Ok(());
    }
    check_room(state)?;
    let (server, community) = discover(&origin).await?;
    let held = choose(state, &server).await?;
    let session = make_session(state, &id, origin.clone(), held, community)?;
    let won = {
        let mut registry = state.registry.lock().map_err(|_| "busy")?;
        match registry.add(origin) {
            Ok(_) => {
                if let Some(slot) = registry.slot_mut(&id) {
                    slot.restart();
                    slot.settle(Phase::Ready(Arc::clone(&session)));
                }
                registry.activate(&id);
                true
            }
            // An overlapping call for the same address got in first and owns
            // the slot; this session is the spare.
            Err(AddError::Known(_)) => false,
            Err(full) => return Err(full.to_string()),
        }
    };
    if !won {
        session.sockets.close_all();
        offer_invite(&app, &id, invite.as_deref());
        bring_forward(&app, &id);
        return Ok(());
    }
    if !state.keep_if_held(&id, &session) {
        remove_server(&app, &id).await;
        return Err(crate::web_client::WITHDRAWN.into());
    }
    let existed = app.get_webview(&instance_label(&id)).is_some();
    if let Err(why) = open_page(&app, &id, &session, invite.as_deref()).await {
        remove_server(&app, &id).await;
        return Err(why);
    }
    if existed {
        offer_invite(&app, &id, invite.as_deref());
    }
    changed(&app);
    focus_front(&app);
    Ok(())
}

fn at_the_door(path: &str) -> bool {
    matches!(path, "/" | "/login" | "/index.html") || path.starts_with("/invite/")
}

/// Opens the join screen of a server that is already in the list, when its
/// page has not gone into the app yet.
fn offer_invite(app: &tauri::AppHandle, id: &str, code: Option<&str>) {
    let Some(code) = code else {
        return;
    };
    let Some(page) = app.get_webview(&instance_label(id)) else {
        return;
    };
    let inside = page.url().is_ok_and(|url| !at_the_door(url.path()));
    if !inside {
        let _ = page.navigate(crate::web_client::join_page(Some(code)));
    }
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

/// The signed-in member's server-wide notification level on one server, for the
/// rail's menu. Unsupported (an older server, or one not ready) is an answer,
/// not an error, so the menu simply leaves the entries out.
#[tauri::command]
pub async fn rail_notify_get(
    webview: Webview,
    id: String,
) -> Result<super::rail_notify::NotifyState, String> {
    use super::rail_notify::{HEALTH_PATH, NotifyState, SETTINGS_PATH, parse_settings, supports};
    let app = from_rail(&webview)?;
    let session = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id);
    let Some(session) = session else {
        return Ok(NotifyState::unsupported());
    };
    let ask = |path: &'static str| {
        let session = session.clone();
        async move {
            let resp = forward(&session.instance, "GET", path, &[], Vec::new()).await;
            (resp.status == 200).then_some(resp.body)
        }
    };
    let supported = ask(HEALTH_PATH).await.is_some_and(|body| supports(&body));
    if !supported {
        return Ok(NotifyState::unsupported());
    }
    Ok(ask(SETTINGS_PATH)
        .await
        .and_then(|body| parse_settings(&body))
        .unwrap_or_else(NotifyState::unsupported))
}

/// Sets that level. `suppress_everyone` is read from the server first and kept,
/// so the rail never changes more than the level.
#[tauri::command]
pub async fn rail_notify_set(webview: Webview, id: String, level: String) -> Result<(), String> {
    use super::rail_notify::{SERVER_PATH, put_body};
    let app = from_rail(&webview)?;
    let current = rail_notify_get(webview, id.clone()).await?;
    if !current.supported {
        return Err("This server cannot set a notification level.".into());
    }
    let body = put_body(&level, current.suppress_everyone).ok_or("Unknown notification level.")?;
    let session = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id)
        .ok_or("This server is not connected.")?;
    let headers = [("content-type".to_owned(), "application/json".to_owned())];
    let resp = forward(&session.instance, "PUT", SERVER_PATH, &headers, body).await;
    if resp.status == 204 || resp.status == 200 {
        Ok(())
    } else {
        Err("The server did not accept the change.".into())
    }
}

/// Takes a server out of the list and wipes everything this device holds of
/// it: its stored session (keychain entry and fallback file), its browser
/// storage, its sockets, its toasts and its gaming token.
pub(crate) async fn remove_server(app: &tauri::AppHandle, id: &str) {
    let trusted = app.state::<Trusted>();
    let slot = trusted.registry.lock().ok().and_then(|mut r| r.remove(id));
    let Some(slot) = slot else {
        return;
    };
    match &slot.phase {
        Phase::Ready(session) => {
            session.sockets.close_all();
            session.instance.jar.forget();
        }
        _ => forget_stored_session(&trusted, &slot.origin, id),
    }
    super::web_gaming::forget(app, id);
    super::web_notify::closed(app, id, &slot.generation);
    if let Some(page) = app.get_webview(&instance_label(id)) {
        let _ = page.clear_all_browsing_data();
        let _ = page.close();
    }
    if let Some(attachment) = app.get_webview_window(&attachment_label(id)) {
        let _ = attachment.close();
    }
    wipe_storage(app, id);
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
    fn the_window_hands_focus_to_the_page_once_per_burst() {
        let start = Instant::now();
        let mut handed = None;
        assert!(handoff_due(&mut handed, start));
        assert!(!handoff_due(&mut handed, start + Duration::from_millis(1)));
        assert!(!handoff_due(
            &mut handed,
            start + FOCUS_HOLD - Duration::from_millis(1)
        ));
        assert!(handoff_due(&mut handed, start + FOCUS_HOLD));
    }

    fn frame(front: &str, pages: &[&str], width: u32) -> Frame {
        Frame {
            width,
            height: 800,
            scale_bits: 1.0f64.to_bits(),
            front: front.to_owned(),
            pages: pages.iter().map(|label| (*label).to_owned()).collect(),
        }
    }

    #[test]
    fn the_pages_are_placed_again_only_when_the_frame_changes() {
        let mut prev = None;
        let first = frame("main", &["main"], 1200);
        assert_eq!(place(&mut prev, &first), Place::ShowFront);
        assert_eq!(place(&mut prev, &first), Place::Same);
        let behind = frame("main", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &behind), Place::KeepFront);
        let switched = frame("i-1", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &switched), Place::ShowFront);
        let mut scaled = switched.clone();
        scaled.scale_bits = 1.25f64.to_bits();
        assert_eq!(place(&mut prev, &scaled), Place::KeepFront);
        scaled.width += 1;
        assert_eq!(place(&mut prev, &scaled), Place::KeepFront);
    }

    #[test]
    fn a_front_page_that_appears_after_the_first_layout_is_shown() {
        let mut prev = None;
        let waiting = frame("i-1", &["main"], 1200);
        assert_eq!(place(&mut prev, &waiting), Place::ShowFront);
        let arrived = frame("i-1", &["main", "i-1"], 1200);
        assert_eq!(place(&mut prev, &arrived), Place::ShowFront);
        assert_eq!(place(&mut prev, &arrived), Place::Same);
    }

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
    fn the_data_store_api_needs_macos_14() {
        assert!(!data_stores_available(Some(12)));
        assert!(!data_stores_available(Some(13)));
        assert!(data_stores_available(Some(14)));
        assert!(data_stores_available(Some(26)));
        assert!(!data_stores_available(None));
    }

    #[test]
    fn a_download_keeps_to_the_folder_and_to_free_names() {
        let dir = std::path::Path::new("/dl");
        let free = |_: &std::path::Path| false;
        assert_eq!(download_target(dir, "a.zip", free), dir.join("a.zip"));
        assert_eq!(
            download_target(dir, "../../etc/x.bin", free),
            dir.join("x.bin")
        );
        assert_eq!(download_target(dir, "..\\y.txt", free), dir.join("y.txt"));
        assert_eq!(download_target(dir, ".hidden", free), dir.join("hidden"));
        assert_eq!(download_target(dir, "", free), dir.join("download"));
        let taken = [dir.join("a.zip"), dir.join("a (2).zip")];
        assert_eq!(
            download_target(dir, "a.zip", |p| taken.iter().any(|t| t == p)),
            dir.join("a (3).zip")
        );
    }

    #[test]
    fn the_first_server_always_runs_and_more_need_separate_storage() {
        assert!(room_for(0));
        assert!(room_with(0, false));
        assert!(!room_with(1, false));
        assert!(room_with(1, true));
    }

    #[test]
    fn the_window_is_titled_with_the_active_server_in_front() {
        use crate::instances::RailInstance;
        let instance = |id: &str, name: &str| RailInstance {
            id: id.into(),
            initial: "L".into(),
            hue: 0,
            name: name.into(),
            status: "ready",
            message: None,
            unread: 0,
            mentions: 0,
        };
        let mut state = RailState {
            active: None,
            adding: false,
            can_add: true,
            instances: vec![instance("a", "Lantern Fox"), instance("b", "b.example")],
        };
        assert_eq!(window_title(&state), TITLE);
        state.active = Some("b".into());
        assert_eq!(window_title(&state), "b.example \u{2014} Mnema Desktop DEV");
        state.active = Some("a".into());
        assert!(window_title(&state).starts_with("Lantern Fox"));
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
                assert_eq!(permissions.len(), 9);
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
