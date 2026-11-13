//! Starting the host: the window system, the custom protocol of the app, the
//! commands the pages may call and the background work.
use super::{
    BUILTIN_BUNDLE, commands,
    dialog::{APP_TITLE, Dialog, show_dialog},
    requests::{handle, into_response},
    rounds::refresh_in_background,
    trusted::Trusted,
};
use tauri::Manager;

pub(crate) fn run() {
    crate::web_tls::install_provider();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(std::sync::Mutex::new(crate::web_gaming::Gaming::default()))
        .register_asynchronous_uri_scheme_protocol("mnema", serve_protocol)
        .invoke_handler(invoke_handler())
        .on_menu_event(on_menu)
        .setup(setup)
        .build(tauri::generate_context!())
        .map(|app| app.run(on_run_event));
    if result.is_err() {
        startup_error();
    }
}

/// Answers a request of the custom protocol on the async runtime.
fn serve_protocol(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
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
}

/// The commands the pages may call. The list must match the capability files
/// (checked by a test).
fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        crate::web_shell::desktop_open_instance,
        commands::desktop_ws_open,
        commands::desktop_ws_send,
        commands::desktop_ws_close,
        crate::web_download::desktop_download,
        crate::web_shell::rail_state,
        crate::web_shell::rail_activate,
        crate::web_shell::rail_add,
        crate::web_shell::rail_cancel_add,
        crate::web_shell::rail_edit,
        crate::rail_menu::rail_menu,
        crate::rail_dialog::rail_dialog,
        crate::rail_dialog::dialog_done,
        crate::web_shell::rail_icon,
        crate::web_shell::rail_remove,
        crate::web_shell::rail_reconnect,
        crate::web_shell::rail_notify_get,
        crate::web_shell::rail_notify_set,
        crate::web_gaming::desktop_gaming_sync,
        crate::web_gaming::desktop_gaming_snapshot,
        crate::web_gaming::desktop_gaming_control,
        crate::web_gaming::desktop_gaming_settings,
        crate::web_gaming::desktop_gaming_candidate,
        crate::web_gaming::desktop_gaming_add_game,
        crate::web_gaming::desktop_gaming_remove_game,
        crate::native_updates::native_update_state,
        crate::native_updates::native_update_action,
        crate::web_notify::desktop_notify_open,
        crate::web_notify::desktop_notify,
        crate::web_notify::desktop_notify_permission,
        crate::web_notify::desktop_notify_request_permission,
        crate::web_notify::desktop_set_badge,
        crate::web_notify::desktop_report_unread
    ]
}

fn on_menu(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    if id == "gaming-settings" {
        let _ = crate::web_gaming::open_settings(app);
    } else if id == crate::native_updates::MENU_ID {
        crate::native_updates::menu_clicked(app);
    } else {
        crate::web_shell::menu_clicked(app, id);
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    crate::storage::adopt_profile_files(app.handle());
    let data_dir = crate::storage::data_dir(app.handle());
    let trusted = Trusted::new(
        data_dir,
        (!BUILTIN_BUNDLE.is_empty()).then_some(BUILTIN_BUNDLE),
    );
    if let Ok(store) = trusted.store.lock() {
        for name in &store.rejected {
            eprintln!(
                "Mnema Desktop: refused web bundle {name}: it does not verify against the pinned keys."
            );
        }
    }
    app.manage(trusted);
    app.manage(crate::web_notify::Notifier::default());
    crate::web_notify::setup(app.handle());
    crate::native_updates::setup(app.handle());
    crate::ui_watch::start(app.handle());
    {
        let _step = crate::ui_watch::ui_guard::doing("start the gaming watcher");
        crate::web_gaming::setup(app.handle());
    }
    {
        // Creating the window starts WebView2, which on a cold machine can take long.
        let _step = crate::ui_watch::ui_guard::doing("create the main window");
        crate::web_shell::create(app.handle())?;
    }
    refresh_in_background(app.handle());
    Ok(())
}

fn on_run_event(app: &tauri::AppHandle, event: tauri::RunEvent) {
    // The Dock icon brings back a window that was closed on macOS.
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Reopen { .. } = &event {
        crate::web_shell::reopen(app);
    }
    let _ = (app, event);
}

/// What the person is told when the window system cannot start, and what to
/// check first on this platform.
pub(super) fn startup_message() -> String {
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
