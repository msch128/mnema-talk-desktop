//! Gaming display/control adapter for the ordinary web sessions. It grants no
//! authentication, cryptographic or media authority. Windows detection/input
//! reuse the existing native foundations; no game injection.
//!
//! Several servers can be open at once, but the overlay, the shortcuts and the
//! sidepeek belong to exactly one: the server that owns the Talk the person is
//! in. Only that server's page may feed them, and only its page receives what
//! they send back.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
#[cfg_attr(not(windows), allow(dead_code))]
#[path = "../../../src/windows.rs"]
mod games;
#[allow(dead_code)]
#[path = "../../../src/shortcuts.rs"]
mod shortcuts;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Chord {
    key: u16,
    alt: bool,
    control: bool,
    shift: bool,
}
impl Chord {
    fn binding(&self) -> Result<shortcuts::Binding, String> {
        shortcuts::Binding::new(
            self.key,
            shortcuts::Modifiers {
                alt: self.alt,
                control: self.control,
                shift: self.shift,
                windows: false,
            },
        )
        .map_err(|_| "Unsupported or reserved shortcut.".into())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    overlay: Chord,
    mute: Chord,
    deafen: Chord,
    ptt: Chord,
    games: Vec<String>,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default = "opacity_default")]
    sidepeek_opacity: u8,
}
fn enabled_default() -> bool {
    true
}
fn opacity_default() -> u8 {
    55
}
impl Default for Settings {
    fn default() -> Self {
        let chord = |key, alt| Chord {
            key,
            alt,
            control: false,
            shift: false,
        };
        Self {
            enabled: true,
            sidepeek_opacity: opacity_default(),
            overlay: chord(0x4d, true),
            mute: chord(0x4e, true),
            deafen: chord(0x44, true),
            ptt: chord(0x20, false),
            games: vec![
                "Wow.exe".into(),
                "WowClassic.exe".into(),
                "GenshinImpact.exe".into(),
                "Wardogs.exe".into(),
                "Wardogs-Win64-Shipping.exe".into(),
            ],
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<(), String> {
        if self.sidepeek_opacity > 100 {
            return Err("Opacity must be between 0 and 100.".into());
        }
        if !self.games.is_empty() && !games::valid_game_names(&self.games) {
            return Err("Enter game executable basenames only.".into());
        }
        let keys = [&self.overlay, &self.mute, &self.deafen, &self.ptt];
        for (i, key) in keys.iter().enumerate() {
            key.binding()?;
            if keys[..i].contains(key) {
                return Err("Shortcuts must be different.".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    id: String,
    name: String,
    speaking: bool,
    muted: bool,
    sharing: bool,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voice {
    clock: f64,
    scope: Option<String>,
    connected: bool,
    account: Option<String>,
    channel: Option<String>,
    muted: bool,
    deafened: bool,
    sharing: bool,
    ptt_mode: bool,
    members: Vec<Member>,
}
impl Voice {
    fn validate(&self) -> Result<(), String> {
        let valid_id = |id: &str| uuid::Uuid::parse_str(id).is_ok();
        if !self.clock.is_finite() || self.clock < 0.0 || self.clock > 9_007_199_254_740_991.0 {
            return Err("Invalid gaming clock.".into());
        }
        if self.members.len() > 256
            || self.members.iter().any(|m| {
                !valid_id(&m.id)
                    || m.name.chars().count() > 80
                    || m.name.chars().any(char::is_control)
            })
        {
            return Err("Invalid gaming roster.".into());
        }
        if self.connected
            && (!self.scope.as_deref().is_some_and(valid_id)
                || !self.account.as_deref().is_some_and(valid_id)
                || !self.channel.as_deref().is_some_and(valid_id)
                || !self
                    .members
                    .iter()
                    .any(|m| Some(m.id.as_str()) == self.account.as_deref()))
        {
            return Err("Own Talk membership required.".into());
        }
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Snapshot {
    voice: Voice,
    settings: Settings,
    overlay: bool,
    active: bool,
}
pub struct Gaming {
    /// The app origin the sessions are bound to (`local_origin` of their pages).
    origin: Option<String>,
    /// The bridge token of each server's page, by server id.
    tokens: HashMap<String, String>,
    /// The server that owns the Talk gaming is attached to.
    owner: Option<String>,
    voice: Voice,
    updated: Instant,
    settings: Settings,
    overlay: bool,
    active: bool,
    previous: [bool; 4],
    ptt_down: bool,
    ptt_armed: bool,
    last_tick: Instant,
    #[cfg(windows)]
    game: Option<GameLease>,
}
impl Default for Gaming {
    fn default() -> Self {
        Self {
            origin: None,
            tokens: HashMap::new(),
            owner: None,
            voice: Voice::default(),
            updated: Instant::now(),
            settings: Settings::default(),
            overlay: false,
            active: false,
            previous: [true; 4],
            ptt_down: false,
            ptt_armed: false,
            last_tick: Instant::now(),
            #[cfg(windows)]
            game: None,
        }
    }
}
fn local(window: &WebviewWindow) -> bool {
    window.url().is_ok_and(|u| super::web_client::bundled(&u))
}
impl Gaming {
    /// A page reports its Talk. The page must hold the token of its own
    /// server, and while another server's Talk is live that one keeps gaming:
    /// this page's state is ignored, not mixed in.
    fn accept_sync(
        &mut self,
        server: &str,
        token: &str,
        origin: Option<&str>,
        voice: Voice,
    ) -> Result<(), String> {
        let token_ok = !token.is_empty() && self.tokens.get(server).is_some_and(|t| t == token);
        if !token_ok || origin.is_none() || origin != self.origin.as_deref() {
            return Err("Wrong gaming session.".into());
        }
        voice.validate()?;
        let held_elsewhere = self.owner.as_deref().is_some_and(|o| o != server)
            && self.voice.connected
            && self.updated.elapsed() < Duration::from_secs(2);
        if held_elsewhere || (!voice.connected && self.owner.as_deref() != Some(server)) {
            return Ok(());
        }
        if !voice.connected
            || voice.account != self.voice.account
            || voice.channel != self.voice.channel
            || voice.scope != self.voice.scope
        {
            self.overlay = false;
            self.active = false;
            self.previous = [true; 4];
            self.ptt_down = false;
            self.ptt_armed = false;
        }
        self.owner = voice.connected.then(|| server.to_owned());
        self.voice = voice;
        self.updated = Instant::now();
        Ok(())
    }
}
#[tauri::command]
pub fn desktop_gaming_sync(
    webview: Webview,
    state: tauri::State<'_, Mutex<Gaming>>,
    token: String,
    voice: Voice,
) -> Result<(), String> {
    let origin =
        super::web_client::local_origin(&webview.url().map_err(|_| "Gaming unavailable.")?);
    let Some(server) = super::instances::id_of_instance_label(webview.label()) else {
        return Err("Wrong gaming session.".into());
    };
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    g.accept_sync(server, &token, origin.as_deref(), voice)
}
#[tauri::command]
pub fn desktop_gaming_snapshot(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
) -> Result<Snapshot, String> {
    if !matches!(
        window.label(),
        "sidepeek" | "gaming-overlay" | "gaming-settings"
    ) || !local(&window)
    {
        return Err("Wrong gaming surface.".into());
    }
    let g = state.lock().map_err(|_| "Gaming unavailable.")?;
    let active = window.label() != "gaming-settings"
        && g.settings.enabled
        && g.active
        && g.updated.elapsed() < Duration::from_secs(2)
        && g.voice.connected;
    Ok(Snapshot {
        voice: if active {
            g.voice.clone()
        } else {
            Voice::default()
        },
        settings: g.settings.clone(),
        overlay: g.overlay && active,
        active,
    })
}
struct Command {
    created: Instant,
    issued: f64,
    /// The server the command goes back to, and its page's token.
    owner: Option<String>,
    token: String,
    origin: Option<String>,
    scope: Option<String>,
    action: String,
    focus: bool,
}
impl Command {
    fn new(g: &Gaming, action: &str) -> Self {
        Self {
            created: Instant::now(),
            issued: g.voice.clock + g.updated.elapsed().as_secs_f64() * 1000.0,
            owner: g.owner.clone(),
            token: g
                .owner
                .as_ref()
                .and_then(|o| g.tokens.get(o))
                .cloned()
                .unwrap_or_default(),
            origin: g.origin.clone(),
            scope: g.voice.scope.clone(),
            action: action.into(),
            focus: action == "stream" && !g.voice.sharing,
        }
    }
    fn dispatch(self, app: &tauri::AppHandle) {
        let Self {
            created,
            issued,
            owner,
            token,
            origin,
            scope,
            action,
            focus,
        } = self;
        let detail = serde_json::json!({"token":token,"scope":scope,"action":action,"issued":issued,"expires":issued+150.0});
        let next = app.clone();
        let _ = app.run_on_main_thread(move || {
            let shared = next.state::<Mutex<Gaming>>();
            let Ok(current) = shared.lock() else {
                return;
            };
            let token_current = owner
                .as_ref()
                .and_then(|o| current.tokens.get(o))
                .is_some_and(|t| *t == token);
            if !token_current
                || current.owner != owner
                || current.origin != origin
                || (action != "ptt-release"
                    && (created.elapsed() >= Duration::from_millis(150)
                        || (action == "ptt-press" && !current.ptt_down)
                        || current.voice.scope != scope
                        || !current.active
                        || current.updated.elapsed() >= Duration::from_secs(2)))
            {
                return;
            }
            drop(current);
            let Some(server) = owner else {
                return;
            };
            if let Some(page) = next.get_webview(&super::instances::instance_label(&server)) {
                if origin.is_none()
                    || !page
                        .url()
                        .is_ok_and(|u| super::web_client::local_origin(&u) == origin)
                {
                    return;
                }
                let _ = page.eval(format!(
                "window.dispatchEvent(new CustomEvent('mnema-gaming-command',{{detail:{detail}}}));"
            ));
                if focus {
                    super::web_shell::bring_forward(&next, &server);
                }
            }
        });
    }
}
#[tauri::command]
pub fn desktop_gaming_control(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    action: String,
) -> Result<(), String> {
    if window.label() == "gaming-settings" && local(&window) && action == "close" {
        return window.close().map_err(|_| "Cannot close settings.".into());
    }
    if window.label() != "gaming-overlay" || !local(&window) {
        return Err("Wrong gaming surface.".into());
    }
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    if !g.active || !g.voice.connected || g.updated.elapsed() >= Duration::from_secs(2) {
        return Err("No current Talk connection.".into());
    }
    if action == "close" {
        g.overlay = false;
        return Ok(());
    }
    if !matches!(action.as_str(), "mute" | "deafen" | "stream" | "ptt-mode") {
        return Err("Unknown gaming control.".into());
    }
    let command = Command::new(&g, &action);
    if action == "stream" && !g.voice.sharing {
        g.overlay = false;
    }
    drop(g);
    command.dispatch(window.app_handle());
    Ok(())
}
#[tauri::command]
pub fn desktop_gaming_settings(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    settings: Settings,
) -> Result<(), String> {
    if !matches!(window.label(), "gaming-overlay" | "gaming-settings") || !local(&window) {
        return Err("Wrong gaming surface.".into());
    }
    settings.validate()?;
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    let dir = window
        .app_handle()
        .path()
        .app_config_dir()
        .map_err(|_| "Settings unavailable.")?;
    std::fs::create_dir_all(&dir).map_err(|_| "Cannot save settings.")?;
    let data = serde_json::to_vec(&settings).map_err(|_| "Invalid settings.")?;
    std::fs::write(dir.join("gaming.json"), data).map_err(|_| "Cannot save settings.")?;
    let command = Command::new(&g, "ptt-release");
    g.ptt_down = false;
    g.ptt_armed = false;
    g.previous = [true; 4];
    if !settings.enabled {
        g.active = false;
        g.overlay = false;
    }
    g.settings = settings;
    drop(g);
    command.dispatch(window.app_handle());
    Ok(())
}
/// Hands a server's page the token that lets it feed gaming.
pub fn bind(app: &tauri::AppHandle, server: &str) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.origin = super::web_client::local_origin(&super::web_client::app_url());
        g.tokens.insert(server.to_owned(), token.clone());
        if g.owner.as_deref() == Some(server) {
            g.release();
        }
    }
    token
}
impl Gaming {
    /// Nobody owns the Talk: no overlay, nothing held.
    fn release(&mut self) {
        let keep = (self.origin.clone(), std::mem::take(&mut self.tokens));
        *self = Gaming {
            settings: self.settings.clone(),
            origin: keep.0,
            tokens: keep.1,
            ..Gaming::default()
        };
    }
}
/// A server's page is gone (its server left the list): it no longer has a
/// token, and gaming lets go of it if it owned the Talk.
pub fn forget(app: &tauri::AppHandle, server: &str) {
    let owned = app.state::<Mutex<Gaming>>().lock().is_ok_and(|mut g| {
        g.tokens.remove(server);
        let owned = g.owner.as_deref() == Some(server);
        if owned {
            g.release();
        }
        owned
    });
    if owned {
        close_surfaces(app);
    }
}
/// The window is closing: everything lets go.
pub fn clear(app: &tauri::AppHandle) {
    if let Ok(mut g) = app.state::<Mutex<Gaming>>().lock() {
        g.tokens.clear();
        g.release();
    }
    close_surfaces(app);
}
fn close_surfaces(app: &tauri::AppHandle) {
    for label in ["sidepeek", "gaming-overlay", "gaming-settings"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.close();
        }
    }
}
/// The entries of the menu bar that are not about servers: gaming settings and
/// the client's own update.
pub fn menu_items(
    app: &tauri::AppHandle,
) -> tauri::Result<(
    tauri::menu::MenuItem<tauri::Wry>,
    tauri::menu::MenuItem<tauri::Wry>,
)> {
    let item =
        tauri::menu::MenuItem::with_id(app, "gaming-settings", "Gaming", true, None::<&str>)?;
    Ok((item, super::native_updates::menu_item(app)?))
}
pub fn open_settings(app: &tauri::AppHandle) -> tauri::Result<()> {
    let window = surface(app, "gaming-settings", true)?;
    window.show()?;
    window.set_focus()
}
fn surface(
    app: &tauri::AppHandle,
    label: &str,
    interactive: bool,
) -> Result<WebviewWindow, tauri::Error> {
    if let Some(w) = app.get_webview_window(label) {
        return Ok(w);
    }
    let builder = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::CustomProtocol(super::web_client::app_url()),
    )
    .use_https_scheme(true)
    .title(if interactive {
        "Mnema Gaming DEV"
    } else {
        "Mnema Sidepeek DEV"
    })
    .initialization_script(super::web_client::identity_script(app, "surface", None))
    .initialization_script(format!(
        "Object.defineProperty(window,'__MNEMA_GAMING_SURFACE__',{{value:'{label}'}});"
    ));
    let builder = if crate::web_permissions::probe_enabled() {
        builder.initialization_script(crate::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    let w = crate::web_permissions::with_probe_args(
        app,
        label,
        super::web_shell::host_profile_window(builder, app),
    )
    .inner_size(
        if interactive { 420.0 } else { 240.0 },
        if interactive { 650.0 } else { 430.0 },
    )
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .focused(false)
    .focusable(interactive)
    .on_navigation(super::web_client::bundled)
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .on_permission_request(super::web_client::allow_permission)
    .build()?;
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        let color: u32 = 0xfffffffe; // DWMWA_COLOR_NONE, Windows 11 border only.
        // SAFETY: a live owned HWND and a sized color buffer; unsupported OS errors are ignored.
        unsafe {
            windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd.0 as _,
                windows_sys::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR as u32,
                (&color as *const u32).cast(),
                std::mem::size_of_val(&color) as u32,
            );
        }
    }
    let owner = app.clone();
    w.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. })
            && let Ok(mut g) = owner.state::<Mutex<Gaming>>().lock()
        {
            g.overlay = false;
        }
    });
    if !interactive {
        let _ = w.set_ignore_cursor_events(true);
    }
    Ok(w)
}
#[cfg(windows)]
struct GameLease {
    hwnd: usize,
    pid: u32,
    process: usize,
}
#[cfg(windows)]
impl GameLease {
    fn new(hwnd: usize, pid: u32) -> Option<Self> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!process.is_null()).then_some(Self {
            hwnd,
            pid,
            process: process as usize,
        })
    }
    fn live(&self) -> bool {
        use windows_sys::Win32::{
            System::Threading::GetExitCodeProcess,
            UI::WindowsAndMessaging::{
                GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
            },
        };
        let mut pid = 0;
        let mut code = 0;
        let hwnd = self.hwnd as *mut std::ffi::c_void;
        unsafe {
            IsWindow(hwnd) != 0
                && IsWindowVisible(hwnd) != 0
                && IsIconic(hwnd) == 0
                && GetWindowThreadProcessId(hwnd, &mut pid) != 0
                && pid == self.pid
                && GetExitCodeProcess(self.process as *mut std::ffi::c_void, &mut code) != 0
                && code == 259
        }
    }
}
#[cfg(windows)]
impl Drop for GameLease {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.process as *mut std::ffi::c_void);
        }
    }
}
#[cfg(windows)]
fn native_sample(
    g: &mut Gaming,
    app: &tauri::AppHandle,
) -> (bool, [bool; 4], Option<tauri::PhysicalPosition<i32>>) {
    use windows_sys::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::{
            Input::KeyboardAndMouse::GetAsyncKeyState,
            WindowsAndMessaging::{
                GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            },
        },
    };
    let hwnd = unsafe { GetForegroundWindow() };
    let own_overlay = app
        .get_webview_window("gaming-overlay")
        .and_then(|w| w.hwnd().ok())
        .is_some_and(|h| h.0 == hwnd);
    let probe = games::inspect_foreground(&g.settings.games);
    let detected = probe.game.is_some()
        && probe
            .native_identity
            .is_some_and(|(window, _)| window == hwnd as usize);
    if detected {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if probe.native_identity != Some((hwnd as usize, pid)) {
            return (false, [false; 4], None);
        }
        if !g
            .game
            .as_ref()
            .is_some_and(|game| game.hwnd == hwnd as usize && game.pid == pid && game.live())
        {
            g.game = GameLease::new(hwnd as usize, pid);
        }
    }
    let game_live = g.game.as_ref().is_some_and(GameLease::live);
    let active = game_live && (detected || (g.overlay && g.active && own_overlay));
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
        return (false, [false; 4], None);
    }
    let down = |key: u16| unsafe { GetAsyncKeyState(i32::from(key)) } < 0;
    let alt = down(0x12);
    let control = down(0x11);
    let shift = down(0x10);
    let win = down(0x5b) || down(0x5c);
    let keys = [
        &g.settings.overlay,
        &g.settings.mute,
        &g.settings.deafen,
        &g.settings.ptt,
    ]
    .map(|c| !win && c.alt == alt && c.control == control && c.shift == shift && down(c.key));
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let position = if detected && unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        Some(tauri::PhysicalPosition::new(
            info.rcMonitor.left + 12,
            info.rcMonitor.top + (info.rcMonitor.bottom - info.rcMonitor.top) / 4,
        ))
    } else {
        None
    };
    (
        active && hwnd == unsafe { GetForegroundWindow() },
        keys,
        position,
    )
}
#[cfg(not(windows))]
fn native_sample(
    _g: &mut Gaming,
    _app: &tauri::AppHandle,
) -> (bool, [bool; 4], Option<tauri::PhysicalPosition<i32>>) {
    (false, [false; 4], None)
}
impl Gaming {
    // Pure state transition: native UI effects and command dispatch happen only
    // after the caller releases the shared state lock.
    fn advance(&mut self, now: Instant, game: bool, keys: [bool; 4]) -> Vec<Command> {
        let mut commands = Vec::new();
        if now.duration_since(self.last_tick) > Duration::from_millis(250) {
            self.previous = keys;
            self.ptt_armed = false;
        }
        self.last_tick = now;
        let active = self.settings.enabled
            && game
            && self.origin.is_some()
            && self.voice.connected
            && self.updated.elapsed() < Duration::from_secs(2);
        if !active {
            self.overlay = false;
            self.previous = keys;
            if self.ptt_down {
                commands.push(Command::new(self, "ptt-release"));
            }
            self.ptt_down = false;
            self.ptt_armed = false;
        } else {
            if keys[0] && !self.previous[0] {
                self.overlay = !self.overlay;
            }
            for (i, action) in [(1, "mute"), (2, "deafen")] {
                if keys[i] && !self.previous[i] {
                    commands.push(Command::new(self, action));
                }
            }
            if !keys[3] {
                self.ptt_armed = true;
            }
            if self.voice.ptt_mode && !self.overlay && self.ptt_armed && keys[3] {
                commands.push(Command::new(self, "ptt-press"));
                self.ptt_down = true;
            } else if self.ptt_down {
                commands.push(Command::new(self, "ptt-release"));
                self.ptt_down = false;
            }
            self.previous = keys;
        }
        self.active = active;
        commands
    }
}
fn tick(app: &tauri::AppHandle) {
    let shared = app.state::<Mutex<Gaming>>();
    let Ok(mut g) = shared.lock() else {
        return;
    };
    let now = Instant::now();
    let fresh =
        g.settings.enabled && g.voice.connected && g.updated.elapsed() < Duration::from_secs(2);
    let (game, keys, position) = if fresh {
        native_sample(&mut g, app)
    } else {
        (false, [false; 4], None)
    };
    let commands = g.advance(now, game, keys);
    let active = g.active;
    let overlay = g.overlay;
    drop(g);
    for command in commands {
        command.dispatch(app);
    }
    for (label, visible) in [("sidepeek", active), ("gaming-overlay", active && overlay)] {
        if visible {
            if let Ok(w) = surface(app, label, label == "gaming-overlay") {
                if let Some(p) = position {
                    let _ = w.set_position(p);
                }
                if !w.is_visible().unwrap_or(false) {
                    let _ = w.show();
                    if label == "gaming-overlay" {
                        let _ = w.set_focus();
                    }
                }
            }
        } else if let Some(w) = app.get_webview_window(label) {
            let _ = w.hide();
        }
    }
}
/// Opens the three gaming windows so the WebView2 probe can see their origin
/// and that they are refused a microphone. Production never calls this.
pub fn open_probe_surfaces(app: &tauri::AppHandle) {
    for (label, interactive) in [
        ("sidepeek", false),
        ("gaming-overlay", true),
        ("gaming-settings", true),
    ] {
        let _ = surface(app, label, interactive);
    }
}

pub fn setup(app: &tauri::AppHandle) {
    if let Ok(dir) = app.path().app_config_dir()
        && let Ok(bytes) = std::fs::read(dir.join("gaming.json"))
        && bytes.len() <= 16384
        && let Ok(settings) = serde_json::from_slice::<Settings>(&bytes)
        && settings.validate().is_ok()
        && let Ok(mut g) = app.state::<Mutex<Gaming>>().lock()
    {
        g.settings = settings;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        loop {
            std::thread::sleep(Duration::from_millis(40));
            if queued.swap(true, std::sync::atomic::Ordering::AcqRel) {
                continue;
            }
            let next = app.clone();
            let flag = queued.clone();
            if app
                .run_on_main_thread(move || {
                    tick(&next);
                    flag.store(false, std::sync::atomic::Ordering::Release);
                })
                .is_err()
            {
                break;
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_keep_bindings_and_new_defaults() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("enabled");
        value.as_object_mut().unwrap().remove("sidepeek_opacity");
        value["overlay"]["key"] = 75.into();
        let restored: Settings = serde_json::from_value(value).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.sidepeek_opacity, 55);
        assert_eq!(restored.overlay.key, 75);
        assert!(restored.validate().is_ok());
        let mut invalid = restored;
        invalid.sidepeek_opacity = 101;
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn global_disable_hides_surfaces_and_releases_ptt_without_reopening_held_key() {
        let mut g = Gaming {
            origin: Some("https://example.invalid".into()),
            ..Gaming::default()
        };
        g.voice.connected = true;
        g.voice.ptt_mode = true;
        let now = Instant::now();
        g.advance(now, true, [false; 4]);
        g.advance(now, true, [false, false, false, true]);
        assert!(g.ptt_down);
        g.overlay = true;
        g.settings.enabled = false;
        let commands = g.advance(now, true, [true; 4]);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].action, "ptt-release");
        assert!(!g.active);
        assert!(!g.overlay);
        g.settings.enabled = true;
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.ptt_down);
        assert!(!g.overlay);
    }
    #[test]
    fn reject_conflicting_reserved_and_path_bindings() {
        let mut s = Settings::default();
        assert!(s.validate().is_ok());
        s.games.clear();
        assert!(s.validate().is_ok());
        s.mute = s.overlay.clone();
        assert!(s.validate().is_err());
        s = Settings::default();
        s.overlay.key = 0x73;
        assert!(s.validate().is_err());
        s = Settings::default();
        s.games = vec!["../game.exe".into()];
        assert!(s.validate().is_err());
    }
    #[test]
    fn gaming_controls_require_fresh_own_voice_game_and_new_key_edges() {
        let mut g = Gaming {
            origin: Some("https://example.invalid".into()),
            ..Gaming::default()
        };
        g.voice.connected = true;
        g.voice.ptt_mode = true;
        let now = Instant::now();
        assert!(g.advance(now, false, [false; 4]).is_empty());
        assert!(!g.active);
        assert!(g.advance(now, true, [false; 4]).is_empty());
        assert!(g.active);
        let actions = g.advance(now, true, [true, true, true, false]);
        assert!(g.overlay);
        assert_eq!(
            actions
                .iter()
                .map(|c| c.action.as_str())
                .collect::<Vec<_>>(),
            ["mute", "deafen"]
        );
        assert!(g.advance(now, true, [true, true, true, false]).is_empty());
        g.advance(now, true, [false; 4]);
        g.advance(now, true, [true, false, false, false]);
        assert!(!g.overlay);
        let actions = g.advance(now, true, [false, false, false, true]);
        assert_eq!(actions[0].action, "ptt-press");
        let actions = g.advance(now, false, [false; 4]);
        assert_eq!(actions[0].action, "ptt-release");
        assert!(!g.active);
        assert!(!g.overlay);
        g.updated = now - Duration::from_secs(3);
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.active);
        g.updated = Instant::now();
        g.last_tick = now - Duration::from_secs(1);
        assert!(g.advance(now, true, [true; 4]).is_empty());
        assert!(!g.ptt_armed);
    }
    fn talk(account: &str, channel: &str, scope: &str) -> Voice {
        Voice {
            connected: true,
            scope: Some(scope.into()),
            account: Some(account.into()),
            channel: Some(channel.into()),
            members: vec![Member {
                id: account.into(),
                name: "Player".into(),
                speaking: false,
                muted: false,
                sharing: false,
            }],
            ..Voice::default()
        }
    }

    fn two_servers() -> Gaming {
        let mut g = Gaming {
            origin: Some("mnema://localhost".into()),
            ..Gaming::default()
        };
        g.tokens.insert("a".into(), "token-a".into());
        g.tokens.insert("b".into(), "token-b".into());
        g
    }

    const U1: &str = "11111111-1111-4111-8111-111111111111";
    const U2: &str = "22222222-2222-4222-8222-222222222222";
    const U3: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn a_page_needs_the_token_of_its_own_server_and_the_apps_origin() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        assert!(
            g.accept_sync("a", "token-b", origin, talk(U1, U2, U3))
                .is_err()
        );
        assert!(g.accept_sync("a", "", origin, talk(U1, U2, U3)).is_err());
        assert!(
            g.accept_sync("c", "token-a", origin, talk(U1, U2, U3))
                .is_err()
        );
        assert!(
            g.accept_sync(
                "a",
                "token-a",
                Some("https://evil.example"),
                talk(U1, U2, U3)
            )
            .is_err()
        );
        assert!(
            g.accept_sync("a", "token-a", None, talk(U1, U2, U3))
                .is_err()
        );
        assert!(g.owner.is_none(), "nothing was accepted");
        assert!(
            g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
                .is_ok()
        );
        assert_eq!(g.owner.as_deref(), Some("a"));
    }

    #[test]
    fn gaming_stays_with_the_server_that_owns_the_talk() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
            .unwrap();
        // B reports its own Talk while A's is live: ignored, not mixed in.
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("a"));
        assert_eq!(g.voice.account.as_deref(), Some(U1));
        // B saying "not connected" does not end A's Talk either.
        g.accept_sync("b", "token-b", origin, Voice::default())
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("a"));
        assert!(g.voice.connected);
        // Commands go back to A's page with A's token.
        let command = Command::new(&g, "mute");
        assert_eq!(
            (command.owner.as_deref(), command.token.as_str()),
            (Some("a"), "token-a")
        );
        // A leaves its Talk: gaming lets go, and B may take it.
        g.accept_sync("a", "token-a", origin, Voice::default())
            .unwrap();
        assert_eq!(g.owner, None);
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(g.owner.as_deref(), Some("b"));
        let command = Command::new(&g, "mute");
        assert_eq!(
            (command.owner.as_deref(), command.token.as_str()),
            (Some("b"), "token-b")
        );
    }

    #[test]
    fn a_stale_owner_does_not_hold_gaming_forever() {
        let mut g = two_servers();
        let origin = Some("mnema://localhost");
        g.accept_sync("a", "token-a", origin, talk(U1, U2, U3))
            .unwrap();
        g.updated = Instant::now() - Duration::from_secs(3);
        g.accept_sync("b", "token-b", origin, talk(U3, U2, U1))
            .unwrap();
        assert_eq!(
            g.owner.as_deref(),
            Some("b"),
            "A stopped reporting, B takes over"
        );
        assert!(!g.overlay && !g.active, "a different Talk starts clean");
    }

    #[test]
    fn letting_go_of_a_server_forgets_its_token_and_keeps_the_settings() {
        let mut g = two_servers();
        g.settings.sidepeek_opacity = 20;
        g.accept_sync("a", "token-a", Some("mnema://localhost"), talk(U1, U2, U3))
            .unwrap();
        g.release();
        assert_eq!(g.owner, None);
        assert!(!g.voice.connected);
        assert_eq!(g.settings.sidepeek_opacity, 20);
        assert_eq!(g.tokens.len(), 2, "release keeps the tokens of live pages");
        assert_eq!(g.origin.as_deref(), Some("mnema://localhost"));
    }

    #[test]
    fn own_membership_is_required() {
        let mut v = Voice {
            connected: true,
            scope: Some(uuid::Uuid::new_v4().to_string()),
            account: Some(uuid::Uuid::new_v4().to_string()),
            channel: Some(uuid::Uuid::new_v4().to_string()),
            ..Voice::default()
        };
        assert!(v.validate().is_err());
        v.members.push(Member {
            id: v.account.clone().unwrap(),
            name: "Player".into(),
            speaking: false,
            muted: false,
            sharing: false,
        });
        assert!(v.validate().is_ok());
        v.members[0].name = "\n".into();
        assert!(v.validate().is_err());
    }
}
