//! The one window around the servers: a rail on the left, and the page of one
//! server (or the page for adding one) next to it.
//!
//! Every server has a webview of its own, at the host's app origin, with its
//! own browser storage (a WebView2 user-data folder on Windows, a data store
//! on macOS 14 and newer) and its own session in `instances`. A page learns
//! nothing of the others: the protocol handler and every command pick the
//! server by the label of the calling webview. The rail is a page of the host,
//! independent of any bundle and of any server; it is the only one that can
mod assets;
mod commands;
mod icons;
mod layout;
mod menu;
mod pages;
mod rail;
mod retry;
mod room;
mod servers;
mod sleep;
mod storage;
mod window;

pub use assets::rail_asset;
pub use commands::*;
pub use icons::*;
pub use menu::menu_clicked;
pub use rail::{bring_forward, counts_changed};
#[cfg(any(feature = "multi-selftest", feature = "real-selftest"))]
pub(crate) use servers::add_server;
pub use servers::bundles_changed;
#[cfg(feature = "multi-selftest")]
pub(crate) use servers::remove_server;
pub use storage::host_profile_window;
#[cfg(feature = "multi-selftest")]
pub(crate) use storage::{open_default_page, wipe_default_storage};
pub use window::create;
pub(crate) use window::host_page_at;
#[cfg(target_os = "macos")]
pub use window::reopen;

/// The rail's width in logical pixels.
pub const RAIL_WIDTH: f64 = 68.0;
pub(crate) const SHELL: &str = "shell";
pub(crate) const RAIL: &str = "rail";
/// The page for adding a server keeps the label of the old instance selector.
const ADD: &str = "main";
/// The Windows startup check looks for a window of this name.
const TITLE: &str = "Mnema Desktop DEV \u{2014} Instance";
const PRODUCT: &str = "Mnema Desktop DEV";
pub(crate) const NEEDS_NEWER_MACOS: &str = "Several servers at once need macOS 14 or newer.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_may_only_be_a_page_of_the_host() {
        // The rail and the adding page are the only host pages with commands;
        // their labels are not labels of a server's page.
        for label in [RAIL, ADD, SHELL] {
            assert!(crate::instances::id_of_instance_label(label).is_none());
        }
    }

    #[test]
    fn the_pages_of_the_host_do_not_share_a_label() {
        assert_ne!(ADD, RAIL);
        assert_ne!(ADD, SHELL);
        assert_ne!(RAIL, SHELL);
    }
}
