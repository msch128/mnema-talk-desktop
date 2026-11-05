//! The trusted desktop client. The web app runs from a signed bundle that this
//! host verified, at the host's own origin; the servers are reached through
//! the host (`web_proxy`, `web_ws`), so they supply data and never code. Each
//! server has its own page and its own session (`instances`, `web_shell`).
//! See mnema-talk `doc/desktop-trusted-client.md`.
mod address;
mod commands;
mod dialog;
mod discover;
mod links;
mod requests;
mod rounds;
mod run;
#[cfg(test)]
mod tests;
mod trusted;

pub(crate) use address::{address, invite_code, join_page};
#[cfg(any(target_os = "macos", test))]
pub(crate) use commands::own_blob;
pub(crate) use commands::{allow_permission, app_api, instance_page};
pub(crate) use discover::{NO_HTTPS, SELECTOR_ONLY, discover};
pub(crate) use links::{Link, confirm_add, invite_decision, navigation_allowed, open_external};
pub(crate) use rounds::choose;
pub(crate) use run::run;
pub(crate) use trusted::{Trusted, WITHDRAWN};

use crate::web_bundle::Held;
use tauri::Manager;
use url::Url;

/// The signed bundle compiled into this executable (empty when the build
/// had none; the client then has to download one before it can start).
static BUILTIN_BUNDLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/builtin-bundle.zip"));

/// Where the app lives. WebView2 shows a custom protocol as
/// `https://<scheme>.localhost`, the other engines as `<scheme>://localhost`.
#[cfg(windows)]
const APP_URL: &str = "https://mnema.localhost/";
#[cfg(not(windows))]
const APP_URL: &str = "mnema://localhost/";

pub fn app_url() -> Url {
    Url::parse(APP_URL).expect("constant URL")
}

/// `scheme://host` of a URL on the app origin. (`url::Origin` is opaque for a
/// custom scheme and never compares equal, so origins are compared as text.)
pub fn local_origin(url: &Url) -> Option<String> {
    let app = app_url();
    (url.scheme() == app.scheme()
        && url.host_str() == app.host_str()
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()))
}

/// An app page: the app origin and one of the app's own routes.
pub(super) fn bundled(url: &Url) -> bool {
    local_origin(url).is_some() && super::local_route::app_path(url.path())
}

/// What this host offers the page beyond the common surface. The web app
/// detects these instead of bumping `web_client_api`; older bundles ignore it.
/// `notify`: toasts and the badge. `unread`: the page reports unread channels
/// and mentions for the server rail. `ws_query`: the socket may ask for
/// conversation events (`/api/ws?dm=1`).
fn capabilities(role: &str) -> &'static [&'static str] {
    if role == "instance" {
        &["notify", "unread", "ws_query"]
    } else {
        &[]
    }
}

/// Announces the verified bundle to the page before any of its scripts run.
/// The app honours it only on the host's own origin. A server's page passes
/// the bundle it was given; every other window runs the one in front.
pub fn identity_script(app: &tauri::AppHandle, role: &str, held: Option<&Held>) -> String {
    let front;
    let held = match held {
        Some(held) => held,
        None => {
            let Some(view) = app.state::<Trusted>().view() else {
                return String::new();
            };
            front = view;
            &front
        }
    };
    let identity = serde_json::json!({
        "version": held.bundle.version.to_string(),
        "source": held.source.as_str(),
        "webClientApi": held.bundle.web_client_api,
        "keyId": held.bundle.key_id,
        "commit": held.bundle.commit,
        "role": role,
        "capabilities": capabilities(role),
    });
    format!(
        "{BUFFERED_BODIES}\nif(window===window.top)Object.defineProperty(window,'__MNEMA_TRUSTED_APP__',{{value:Object.freeze({identity})}});"
    )
}

/// Makes the page read a picked file itself before it sends it: the webviews
/// hand the host no file bodies (WebView2 an empty one). See the file.
pub const BUFFERED_BODIES: &str = include_str!("../page-shim/buffered-bodies.js");

#[cfg(test)]
mod drop_tests {
    /// Dropped files must reach the instance page as HTML5 events, so the
    /// native drop handler of the webview is off for that page. Nothing of the
    /// trusted local UI listens for native drops.
    #[test]
    fn the_instance_page_takes_file_drops_itself() {
        let shell = include_str!("web_shell.rs");
        let start = shell
            .find("fn instance_page(")
            .or_else(|| shell.find("let label = instance_label(id);"))
            .expect("the builder of the instance page");
        let body = &shell[start..];
        let end = body.find("isolate(builder, app, id)").expect("its end");
        assert!(body[..end].contains(".disable_drag_drop_handler()"));
        for (name, source) in [
            ("web_shell", shell),
            ("native_updates", include_str!("native_updates.rs")),
        ] {
            assert!(!source.contains("onDragDropEvent"), "{name}");
        }
        let gaming = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web_gaming");
        for entry in std::fs::read_dir(gaming).unwrap().flatten() {
            let source = std::fs::read_to_string(entry.path()).unwrap();
            assert!(
                !source.contains("onDragDropEvent"),
                "{:?}",
                entry.file_name()
            );
        }
        assert!(!include_str!("web_gaming.rs").contains("onDragDropEvent"));
    }
}
