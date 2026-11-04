//! Answering the requests of the webviews: a server's page from its own
//! session and bundle, the host's own pages from the host.
use super::trusted::Trusted;
use crate::instances::{Phase, id_of_attachment_label, id_of_instance_label};
use crate::web_bundle::Refusal;
use crate::web_proxy::{Resp, forward, serve_bundle};
use std::borrow::Cow;
use tauri::Manager;

pub(super) fn into_response(resp: Resp) -> tauri::http::Response<Cow<'static, [u8]>> {
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

/// Why a server's page shows nothing but a sentence: it is still being
/// checked, or it was refused (the instance is too old or too new for any
/// verified bundle, or cannot be reached). Shown in that server's tab only.
fn unavailable(trusted: &Trusted, id: &str) -> Resp {
    let text = trusted
        .registry
        .lock()
        .ok()
        .and_then(|registry| match registry.slot(id)?.phase {
            Phase::Starting => Some("Connecting to the server.".to_owned()),
            Phase::Refused(ref why) => Some(why.clone()),
            Phase::Ready(_) => None,
        })
        .unwrap_or_else(|| "No instance is selected.".into());
    plain(503, &text)
}

/// One request of the page (or attachment window) of a server: its API calls
/// go to that server with that server's cookies, and its files come from the
/// bundle that server was matched with. No other server is reachable from here.
pub(super) async fn page_request(
    trusted: &Trusted,
    id: &str,
    request: tauri::http::Request<Vec<u8>>,
) -> Resp {
    let path = request.uri().path().to_owned();
    let Some(session) = trusted.session(id) else {
        let answer = unavailable(trusted, id);
        return if path == "/api" || path.starts_with("/api/") {
            answer.sealed()
        } else {
            answer
        };
    };
    let method = request.method().as_str().to_owned();
    if path == "/api" || path.starts_with("/api/") {
        let path_and_query = request
            .uri()
            .path_and_query()
            .map_or(path.clone(), |p| p.as_str().to_owned());
        let headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        forward(
            &session.instance,
            &method,
            &path_and_query,
            &headers,
            request.into_body(),
        )
        .await
    } else {
        serve_bundle(&session.held.bundle, &method, &path)
    }
}

/// Answers one request of one webview.
pub(super) async fn handle(
    app: &tauri::AppHandle,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
) -> Resp {
    let trusted = app.state::<Trusted>();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if crate::web_permissions::take_probe(label, &path, request.uri().query()) {
        return plain(204, "");
    }
    let api = path == "/api" || path.starts_with("/api/");
    if let Some(id) = id_of_instance_label(label).or_else(|| id_of_attachment_label(label)) {
        return page_request(&trusted, id, request).await;
    }
    match label {
        // The probe opens an instance-labelled page before any server is
        // selected, so its document still comes from the verified bundle. API
        // calls have no server.
        "instance" if !api && crate::web_permissions::probe_enabled() => match trusted.view() {
            Some(held) => serve_bundle(&held.bundle, &method, &path),
            None => plain(503, "No instance is selected."),
        },
        "instance" => plain(503, "No instance is selected."),
        // The host's own update page; independent of the web bundle.
        "updater" if !api => match crate::native_updates::asset(&path) {
            Some((kind, body)) if method == "GET" || method == "HEAD" => Resp {
                status: 200,
                headers: vec![
                    ("content-type".into(), kind.into()),
                    (
                        "content-security-policy".into(),
                        crate::native_updates::UI_CSP.into(),
                    ),
                    ("x-content-type-options".into(), "nosniff".into()),
                    ("cache-control".into(), "no-store".into()),
                ],
                body: body.as_bytes().to_vec(),
            },
            _ => plain(404, "Not found."),
        },
        // The server rail: the host's own page, independent of the web bundle
        // and of every server.
        "rail" | "rail-dialog" if !api => crate::web_shell::rail_asset(&method, &path),
        "main" | "sidepeek" | "gaming-overlay" | "gaming-settings" if !api => {
            match trusted.view() {
                Some(held) => serve_bundle(&held.bundle, &method, &path),
                None => {
                    let reason = trusted
                        .store
                        .lock()
                        .map(|s| s.empty_reason())
                        .unwrap_or(Refusal::NoBundle);
                    plain(503, &reason.to_string())
                }
            }
        }
        "main" | "rail" | "rail-dialog" | "sidepeek" | "gaming-overlay" | "gaming-settings" => {
            plain(404, "Not found.")
        }
        _ => plain(403, "Forbidden."),
    }
}

pub(super) fn plain(status: u16, text: &str) -> Resp {
    Resp {
        status,
        headers: vec![
            ("content-type".into(), "text/plain; charset=utf-8".into()),
            ("x-content-type-options".into(), "nosniff".into()),
        ],
        body: text.as_bytes().to_vec(),
    }
}
