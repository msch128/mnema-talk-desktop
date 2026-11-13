//! The windows gaming draws: the sidepeek, the overlay and the settings.
use super::state::Gaming;
use std::sync::Mutex;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub(super) fn close_surfaces(app: &tauri::AppHandle) {
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
    Ok((item, crate::native_updates::menu_item(app)?))
}
pub fn open_settings(app: &tauri::AppHandle) -> tauri::Result<()> {
    let window = surface(app, "gaming-settings", true)?;
    window.show()?;
    window.set_focus()
}
pub(super) fn surface(
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
        WebviewUrl::CustomProtocol(crate::web_client::app_url()),
    )
    .use_https_scheme(true)
    .title(if interactive {
        "Mnema Gaming"
    } else {
        "Mnema Sidepeek"
    })
    .initialization_script(crate::web_client::identity_script(app, "surface", None))
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
        crate::web_shell::host_profile_window(builder, app),
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
    .on_navigation(crate::web_client::bundled)
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .on_permission_request(crate::web_client::allow_permission)
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
