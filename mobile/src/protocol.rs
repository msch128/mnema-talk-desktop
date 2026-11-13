//! What the app origin serves: the verified bundle's files, the identity of
//! the bundle for the role the host is in, and `/api/` of the open server
//! through the shared proxy. Requests come from the one webview only.
use crate::host::{Host, MAIN};
use crate::web_bundle::Held;
use crate::web_proxy::{Resp, forward, serve_bundle};
use std::borrow::Cow;
use tauri::Manager;

/// The script that announces the verified bundle. The desktop host gives each
/// webview a fixed initialization script; the phone has one webview that
/// changes role, so the document names this script and the host writes it for
/// the role in force when the page loads (same origin, so the bundle's CSP
/// allows it, and it runs before the bundle's module scripts).
pub const IDENTITY_PATH: &str = "/__mnema/identity.js";

/// Android: requests with a body go over IPC (see `commands::mobile_api`).
#[cfg(target_os = "android")]
pub const IPC_BODIES: &str = include_str!("../page-shim/ipc-bodies.js");

/// The desktop's shim that turns a picked file into bytes before it is sent;
/// WKWebView hands the host a body only when it is bytes already.
const BUFFERED_BODIES: &str = include_str!("../../experimental/app/page-shim/buffered-bodies.js");

pub fn serve(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let label = ctx.webview_label().to_owned();
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let resp = if label == MAIN {
            handle(&app, request).await
        } else {
            plain(403, "Forbidden.")
        };
        responder.respond(into_response(resp));
    });
}

async fn handle(app: &tauri::AppHandle, request: tauri::http::Request<Vec<u8>>) -> Resp {
    let host = app.state::<Host>();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let active = host.active();
    if path == IDENTITY_PATH {
        #[cfg(feature = "mobile-selftest")]
        crate::selftest::identity(
            app,
            if active.is_some() {
                "instance"
            } else {
                "selector"
            },
        );
        return identity(active.as_ref().map(|a| &a.held), &host);
    }
    if path == "/api" || path.starts_with("/api/") {
        let Some(active) = active else {
            return plain(503, "No server is open.").sealed();
        };
        let path_and_query = request
            .uri()
            .path_and_query()
            .map_or(path.clone(), |p| p.as_str().to_owned());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        let resp = forward(
            &active.instance,
            &method,
            &path_and_query,
            &headers,
            request.into_body(),
        )
        .await;
        #[cfg(feature = "mobile-selftest")]
        crate::selftest::proxied(app, &method, &path, resp.status);
        return resp;
    }
    let held = match active {
        Some(active) => active.held.clone(),
        None => match host.picker_bundle() {
            Some(held) => held,
            None => return plain(503, "This app has no verified web app."),
        },
    };
    let resp = with_identity(serve_bundle(&held.bundle, &method, &path));
    #[cfg(feature = "mobile-selftest")]
    crate::selftest::served(app, &path, &resp);
    resp
}

/// The app shell names the identity script first in its head.
fn with_identity(mut resp: Resp) -> Resp {
    let html = resp
        .headers
        .iter()
        .any(|(k, v)| k == "content-type" && v.starts_with("text/html"));
    if !html {
        return resp;
    }
    let tag = format!("<script src=\"{IDENTITY_PATH}\"></script>");
    if let Some(at) = find(&resp.body, b"<head>") {
        let at = at + b"<head>".len();
        resp.body.splice(at..at, tag.into_bytes());
    }
    resp
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The identity of the bundle the page runs, for the role in force: the
/// picker (`selector`) or the page of the open server (`instance`). The web
/// app honours it only on the app origin.
fn identity(active: Option<&Held>, host: &Host) -> Resp {
    let (held, role, capabilities): (Option<Held>, &str, &[&str]) = match active {
        Some(held) => (Some(held.clone()), "instance", &["ws_query"]),
        None => (host.picker_bundle(), "selector", &[]),
    };
    let Some(held) = held else {
        return script(String::new());
    };
    let identity = serde_json::json!({
        "version": held.bundle.version.to_string(),
        "source": held.source.as_str(),
        "webClientApi": held.bundle.web_client_api,
        "keyId": held.bundle.key_id,
        "commit": held.bundle.commit,
        "role": role,
        "capabilities": capabilities,
    });
    script(format!(
        "{BUFFERED_BODIES}\nif(window===window.top)Object.defineProperty(window,'__MNEMA_TRUSTED_APP__',{{value:Object.freeze({identity})}});"
    ))
}

fn script(body: String) -> Resp {
    Resp {
        status: 200,
        headers: vec![
            (
                "content-type".into(),
                "text/javascript; charset=utf-8".into(),
            ),
            ("x-content-type-options".into(), "nosniff".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body: body.into_bytes(),
    }
}

pub fn plain(status: u16, text: &str) -> Resp {
    Resp {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("x-content-type-options".into(), "nosniff".into()),
        ],
        body: text.as_bytes().to_vec(),
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
