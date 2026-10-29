#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "web-client")]
mod instances;
#[cfg(feature = "web-client")]
mod local_route;
#[cfg(feature = "web-client")]
mod native_updates;
#[cfg(feature = "web-client")]
mod rail_notify;
#[cfg(feature = "multi-selftest")]
mod selftest_multi;
#[cfg(feature = "web-client")]
mod storage;
#[cfg(feature = "web-client")]
mod ui_watch;
#[cfg(feature = "web-client")]
mod web_bundle;
#[cfg(feature = "web-client")]
mod web_client;
#[cfg(feature = "web-client")]
mod web_gaming;
#[cfg(feature = "web-client")]
mod web_json;
#[cfg(feature = "web-client")]
mod web_notify;
#[cfg(feature = "web-client")]
mod web_permissions;
#[cfg(feature = "web-client")]
mod web_proxy;
#[cfg(feature = "web-client")]
mod web_shell;
#[cfg(feature = "web-client")]
mod web_tls;
#[cfg(feature = "web-client")]
mod web_trust;
#[cfg(feature = "web-client")]
mod web_updates;
#[cfg(feature = "web-client")]
mod web_vault;
#[cfg(feature = "web-client")]
mod web_ws;

#[cfg(feature = "web-client")]
fn main() {
    web_client::run();
}
