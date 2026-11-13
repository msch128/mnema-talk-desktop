//! The window itself: the pages of the host, the window around them, and the
//! end of the program with it.
use super::icons::{show_cached_icon, sweep_icons};
use super::layout::{ShellPlacement, layout, on_window_focused};
use super::menu::menu;
use super::rail::push_rail;
use super::servers::{registry_ids, spawn_start};
use super::storage::{host_profile, sweep_orphans, wipe_default_storage};
use super::{ADD, RAIL, RAIL_WIDTH, SHELL, TITLE};
use crate::web_client::{Trusted, app_url, bundled};
use std::sync::Mutex;
use tauri::{
    LogicalPosition, LogicalSize, Manager, WebviewUrl, webview::WebviewBuilder,
    window::WindowBuilder,
};

fn host_page(app: &tauri::AppHandle, label: &str, role: &str) -> WebviewBuilder<tauri::Wry> {
    host_page_at(app, label, role, WebviewUrl::CustomProtocol(app_url()))
}

/// A page of the host at a place of its own (the dialog of the rail).
pub(crate) fn host_page_at(
    app: &tauri::AppHandle,
    label: &str,
    role: &str,
    url: WebviewUrl,
) -> WebviewBuilder<tauri::Wry> {
    let builder = WebviewBuilder::new(label, url)
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
    let builder = if crate::web_permissions::probe_enabled() && !role.is_empty() {
        builder.initialization_script(crate::web_permissions::PROBE_SCRIPT)
    } else {
        builder
    };
    crate::web_permissions::with_probe_args_child(app, label, host_profile(builder, app))
}

/// Opens the window with the rail and the page for adding a server, and starts
/// the servers of the list.
pub fn create(app: &tauri::AppHandle) -> tauri::Result<()> {
    app.manage(Mutex::new(ShellPlacement::default()));
    app.manage(crate::rail_menu::Open::default());
    app.manage(crate::rail_dialog::Open::default());
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
    if crate::web_permissions::probe_enabled() {
        // A page labelled like the pages of servers, before any server exists,
        // so that the startup check can see how it is treated.
        let _ = window.add_child(
            host_page(app, "instance", "probe").focused(false),
            LogicalPosition::new(RAIL_WIDTH, 0.0),
            LogicalSize::new(1.0, 1.0),
        );
        crate::web_gaming::open_probe_surfaces(app);
    }
    let events = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            layout(&events);
        }
        // Minimising and restoring do not always come with a resize.
        tauri::WindowEvent::Focused(true) => {
            layout(&events);
            on_window_focused(&events);
        }
        tauri::WindowEvent::Focused(false) => layout(&events),
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
    sweep_icons(app);
    for id in registry_ids(app) {
        show_cached_icon(app, &id);
        spawn_start(app, id);
    }
    layout(app);
    push_rail(app);
    super::sleep::watch(app);
    #[cfg(feature = "multi-selftest")]
    crate::selftest_multi::start(app);
    #[cfg(feature = "real-selftest")]
    crate::selftest_real::start(app);
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

/// The window is gone: every connection ends and the program with it.
fn shut_down(app: &tauri::AppHandle) {
    if let Ok(registry) = app.state::<Trusted>().registry.lock() {
        for id in registry.ids() {
            if let Some(session) = registry.session(&id) {
                session.sockets.close_all();
            }
        }
    }
    crate::web_gaming::clear(app);
    app.exit(0);
}
