//! The phone host (Android, iOS), a spike. The trust core is the desktop
//! host's own code, built here unchanged from `experimental/app/src`: the
//! bundle store and its verification, the proxy with the cookie jar, the TLS
//! stack, the vault and the WebSocket relay. This crate adds a shell for one
//! window with one webview. See `doc/mobile.md`.

/// The desktop host's files. The module names the desktop's source folder, so
/// each file and its submodules load exactly as they do there
/// (`web_bundle/store.rs` and so on). The crate root re-exports them under
/// their desktop names, which the files use for each other (`crate::web_bundle`).
/// The desktop host uses more of them than this shell does; what is left
/// unused here is not a defect.
#[path = "../../experimental/app/src"]
#[allow(dead_code, unused_imports)]
mod shared {
    pub mod local_route;
    pub mod rail_icon;
    pub mod web_bundle;
    pub mod web_json;
    pub mod web_proxy;
    pub mod web_tls;
    pub mod web_vault;
    pub mod web_ws;
    // The phone's own versions of two desktop modules, built from their
    // shared files. They sit in here so that the shared files' `pub(super)`
    // items reach them as they reach each other on the desktop.
    #[path = "../../../mobile/src/shared/web_client.rs"]
    pub mod web_client;
    #[path = "../../../mobile/src/shared/web_notify.rs"]
    pub mod web_notify;
}
pub(crate) use shared::{
    local_route, rail_icon, web_bundle, web_client, web_json, web_notify, web_proxy, web_tls,
    web_vault, web_ws,
};

#[cfg(target_os = "android")]
mod android_tls;
mod commands;
mod host;
mod protocol;
#[cfg(feature = "mobile-selftest")]
mod selftest;

/// The entry point the Android activity and the iOS app delegate call.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    host::run();
}
