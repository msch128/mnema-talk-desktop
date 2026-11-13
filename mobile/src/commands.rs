//! What the page may ask of the host. The desktop picks the server by the
//! label of the calling webview; the phone has one webview, so every command
//! checks that the call comes from the app origin and that the host is in the
//! role the command belongs to.
use crate::host::{Active, Host, MAIN};
use crate::web_proxy::forward;
use crate::web_ws::WsEvent;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::sync::Arc;
use tauri::{Manager, Webview};

fn app_page(webview: &Webview) -> bool {
    webview.label() == MAIN && webview.url().is_ok_and(|u| crate::web_client::local(&u))
}

/// The open server, for a call from its page.
fn open_page(webview: &Webview) -> Result<Arc<Active>, String> {
    if !app_page(webview) {
        return Err("Only the page of the server may use this.".into());
    }
    webview
        .app_handle()
        .state::<Host>()
        .active()
        .ok_or_else(|| "No server is open.".into())
}

/// The picker asks to open a server.
#[tauri::command]
pub async fn desktop_open_instance(webview: Webview, address: String) -> Result<(), String> {
    let picker = app_page(&webview) && webview.app_handle().state::<Host>().active().is_none();
    if !picker {
        return Err(crate::web_client::SELECTOR_ONLY.into());
    }
    crate::host::open_server(webview.app_handle(), &address).await
}

#[tauri::command]
pub fn desktop_ws_open(
    webview: Webview,
    path: String,
    on_event: tauri::ipc::Channel<WsEvent>,
) -> Result<u64, String> {
    let active = open_page(&webview)?;
    let dm = crate::web_ws::socket_request(&path)?;
    crate::web_ws::open(
        &active.sockets,
        Arc::clone(&active.instance),
        dm,
        move |event| on_event.send(event).is_ok(),
    )
}

#[tauri::command]
pub fn desktop_ws_send(webview: Webview, id: u64, data: String) -> Result<(), String> {
    open_page(&webview)?.sockets.send(id, data)
}

#[tauri::command]
pub fn desktop_ws_close(webview: Webview, id: u64) -> Result<(), String> {
    open_page(&webview)?.sockets.close(id);
    Ok(())
}

#[derive(Serialize)]
pub struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    /// Base64, as the IPC carries text.
    body: String,
}

/// One request of the page that carries a body, which Android's WebView does
/// not hand to the custom scheme. It goes through the same proxy function as
/// every other request, with its rules for paths, headers, caps and cookies.
#[tauri::command]
pub async fn mobile_api(
    webview: Webview,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
) -> Result<Answer, String> {
    let active = open_page(&webview)?;
    if !path.starts_with("/api/") {
        return Err("Only /api/ is forwarded.".into());
    }
    let body = STANDARD
        .decode(body)
        .map_err(|_| "The body is not base64.")?;
    let resp = forward(&active.instance, &method, &path, &headers, body).await;
    Ok(Answer {
        status: resp.status,
        headers: resp.headers,
        body: STANDARD.encode(resp.body),
    })
}
