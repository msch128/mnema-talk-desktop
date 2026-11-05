//! What the pages of the host may ask of it: the socket commands of a server's
//! page, permissions, and which addresses count as the app's own.
use super::{local_origin, trusted::Trusted};
use crate::instances::{Session, id_of_instance_label};
use crate::web_ws::WsEvent;
use std::sync::Arc;
use tauri::{Manager, Webview};
use url::Url;

/// The server whose page made a call, if the call comes from the app's own
/// origin in the page of a server that is running. Every command that acts for
/// a server derives the server from here and from nowhere else: the label of
/// the calling webview, never an argument.
pub(crate) fn instance_page(webview: &Webview) -> Result<Arc<Session>, String> {
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
pub(super) fn desktop_ws_open(
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
pub(super) fn desktop_ws_send(webview: Webview, id: u64, data: String) -> Result<(), String> {
    instance_page(&webview)?.sockets.send(id, data)
}

#[tauri::command]
pub(super) fn desktop_ws_close(webview: Webview, id: u64) -> Result<(), String> {
    instance_page(&webview)?.sockets.close(id);
    Ok(())
}

pub(crate) fn allow_permission(
    webview: tauri::Webview<tauri::Wry>,
    kind: tauri::webview::PermissionKind,
) -> tauri::webview::PermissionResponse {
    let url = webview.url().ok();
    crate::web_permissions::decide(webview.label(), url.as_ref(), kind)
}

/// A path the proxy serves for a server, on the app origin.
pub(crate) fn app_api(url: &Url) -> bool {
    local_origin(url).is_some() && url.path().starts_with("/api/")
}

/// A `blob:` address the page made itself (`blob:<app origin>/<id>`), as the
/// recovery codes' download uses.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn own_blob(url: &Url) -> bool {
    url.scheme() == "blob"
        && Url::parse(url.path()).is_ok_and(|inner| local_origin(&inner).is_some())
}
