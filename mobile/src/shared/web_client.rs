//! The parts of the desktop's `web_client` that the phone shares: reading an
//! address a person typed or pasted, and asking a server what it is. The app
//! origin is the phone's own.
#[allow(dead_code)]
#[path = "../../../experimental/app/src/web_client/address.rs"]
mod address;
#[allow(dead_code)]
#[path = "../../../experimental/app/src/web_client/discover.rs"]
mod discover;

pub(crate) use address::address;
pub(crate) use discover::{NO_HTTPS, SELECTOR_ONLY, discover};

use url::Url;

/// Where the app lives. Android's WebView shows a custom protocol as
/// `https://<scheme>.localhost` (as WebView2 does on Windows), WKWebView as
/// `<scheme>://localhost` (as on macOS). The web app accepts both forms.
#[cfg(target_os = "android")]
const APP_URL: &str = "https://mnema.localhost/";
#[cfg(not(target_os = "android"))]
const APP_URL: &str = "mnema://localhost/";

pub fn app_url() -> Url {
    Url::parse(APP_URL).expect("constant URL")
}

/// Whether a URL is on the app origin (compared as text: `url::Origin` is
/// opaque for a custom scheme).
pub fn local(url: &Url) -> bool {
    let app = app_url();
    url.scheme() == app.scheme()
        && url.host_str() == app.host_str()
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

/// A page the webview may show: the app origin and one of the app's routes.
pub fn bundled(url: &Url) -> bool {
    local(url) && crate::local_route::app_path(url.path())
}
