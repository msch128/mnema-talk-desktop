//! The update window: the host's own page, its content security policy and the
//! commands the page calls.
use super::check::check;
use super::install::{install, skip};
use super::logic::release_page;
use super::state::{Mode, NativeUpdates, Phase, Snapshot};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

const WINDOW: &str = "updater";
const UI_PREFIX: &str = "/__update/";

/// The window is the host's own page, served from the executable and not from
/// the web bundle, so it also works when no bundle is usable.
pub fn open_window<R: Runtime>(app: &AppHandle<R>, focus: bool) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW) {
        if focus {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return Ok(());
    }
    let url = ui_base().join("index.html").expect("constant path");
    crate::web_permissions::with_probe_args(
        app,
        WINDOW,
        crate::storage::window_on_default_storage(
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::CustomProtocol(url))
                .use_https_scheme(true)
                .title("Mnema Desktop DEV \u{2014} Update")
                .inner_size(440.0, 320.0)
                .resizable(false)
                .maximizable(false)
                .focused(focus)
                .on_navigation(ui_url)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_permission_request(|webview, kind| {
                    let url = webview.url().ok();
                    crate::web_permissions::decide(webview.label(), url.as_ref(), kind)
                }),
        ),
    )
    .build()?;
    Ok(())
}

/// Menu entry: show the window and look again.
pub fn menu_clicked<R: Runtime>(app: &AppHandle<R>) {
    let _ = open_window(app, true);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let busy_or_waiting = {
            let state = app.state::<NativeUpdates>();
            matches!(
                state.snapshot().phase,
                Phase::Available | Phase::Ready | Phase::Downloading | Phase::Installing
            )
        };
        if !busy_or_waiting {
            check(&app, true).await;
        }
    });
}

pub(super) fn ui_base() -> Url {
    crate::web_client::app_url()
        .join(UI_PREFIX.trim_start_matches('/'))
        .expect("constant path")
}

/// Only the update page itself may be shown in the update window.
pub fn ui_url(url: &Url) -> bool {
    let base = ui_base();
    url.scheme() == base.scheme()
        && url.host_str() == base.host_str()
        && url.port() == base.port()
        && url.path().starts_with(UI_PREFIX)
}

pub(super) const INDEX: &str = include_str!("../../update-ui/index.html");
pub(super) const STYLE: &str = include_str!("../../update-ui/update.css");
pub(super) const SCRIPT: &str = include_str!("../../update-ui/update.js");

/// Body and content type of an update page file, for the `updater` window.
pub fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path.strip_prefix(UI_PREFIX)? {
        "" | "index.html" => Some(("text/html; charset=utf-8", INDEX)),
        "update.css" => Some(("text/css; charset=utf-8", STYLE)),
        "update.js" => Some(("text/javascript; charset=utf-8", SCRIPT)),
        _ => None,
    }
}

pub const UI_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub(super) fn from_update_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == WINDOW && window.url().is_ok_and(|u| ui_url(&u)) {
        Ok(())
    } else {
        Err("Only the update window may use this.".into())
    }
}

#[tauri::command]
pub fn native_update_state(
    window: WebviewWindow,
    state: tauri::State<'_, NativeUpdates>,
) -> Result<Snapshot, String> {
    from_update_window(&window)?;
    Ok(state.snapshot())
}

#[tauri::command]
pub fn native_update_action(
    window: WebviewWindow,
    state: tauri::State<'_, NativeUpdates>,
    action: String,
) -> Result<Snapshot, String> {
    from_update_window(&window)?;
    let app = window.app_handle().clone();
    let portable = state.source.portable;
    match action.as_str() {
        // The portable copy cannot be replaced by an installer.
        "now" | "later" if portable => {
            return Err("A portable copy does not install updates.".into());
        }
        "download_page" => {
            let page = portable
                .then(|| state.snapshot().version)
                .flatten()
                .and_then(|v| release_page(&v));
            if let Some(url) = page {
                // The confirmation dialog waits for an answer.
                std::thread::spawn(move || crate::web_client::open_external(&url));
            }
        }
        // The link of the footer: the page of the offered version, else of the
        // installed one. The address is built here from the fixed prefix.
        "release_page" => {
            let snapshot = state.snapshot();
            let version = snapshot.version.unwrap_or(snapshot.current);
            if let Some(url) = release_page(&version) {
                std::thread::spawn(move || crate::web_client::open_external(&url));
            }
        }
        "check" => {
            tauri::async_runtime::spawn(async move { check(&app, true).await });
        }
        "now" => {
            tauri::async_runtime::spawn(async move { install(&app, Mode::Now).await });
        }
        "later" => {
            tauri::async_runtime::spawn(async move { install(&app, Mode::Later).await });
        }
        "skip" => skip(&app),
        "close" => {
            window.close().map_err(|_| "Cannot close the window.")?;
        }
        _ => return Err("Unknown action.".into()),
    }
    Ok(state.snapshot())
}
