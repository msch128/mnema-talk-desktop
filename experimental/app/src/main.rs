#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(feature = "web-client"))]
include!("native_preview.rs");

#[cfg(feature = "web-client")]
mod local_route;
#[cfg(feature = "web-client")]
mod web_bundle;
#[cfg(feature = "web-client")]
mod web_client;
#[cfg(feature = "web-client")]
mod web_gaming;
#[cfg(feature = "web-client")]
mod web_proxy;
#[cfg(feature = "web-client")]
mod web_updates;
#[cfg(feature = "web-client")]
mod web_ws;

#[cfg(feature = "web-client")]
fn main() {
    web_client::run();
}
