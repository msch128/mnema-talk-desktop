//! Gaming display/control adapter for the ordinary web sessions. It grants no
//! authentication, cryptographic or media authority. Windows detection/input
//! reuse the existing native foundations; no game injection.
//!
//! Several servers can be open at once, but the overlay, the shortcuts and the
//! sidepeek belong to exactly one: the server that owns the Talk the person is
//! in. Only that server's page may feed them, and only its page receives what
//! they send back.
// The modules below are shared with the gaming-logic crate, which tests all of them on every
// OS; this host uses the parts that its platform has, so the rest is dead code here.
#[allow(dead_code)]
#[path = "../../../src/foreground.rs"]
mod foreground;
#[cfg(windows)]
#[path = "../../../src/foreground_windows.rs"]
mod foreground_windows;
#[allow(dead_code)]
#[path = "../../../src/fullscreen.rs"]
mod fullscreen;
#[allow(dead_code)]
#[path = "../../../src/game_candidate.rs"]
mod game_candidate;
#[allow(dead_code)]
#[path = "../../../src/game_identify.rs"]
mod game_identify;
#[allow(dead_code)]
#[path = "../../../src/game_library.rs"]
mod game_library;
#[allow(dead_code)]
#[path = "../../../src/game_library_scan.rs"]
mod game_library_scan;
#[allow(dead_code)]
#[path = "../../../src/game_list.rs"]
mod game_list;
#[allow(dead_code)]
#[path = "../../../src/windows.rs"]
mod games;
#[allow(dead_code)]
#[path = "../../../src/input.rs"]
mod input;
#[cfg(windows)]
#[path = "../../../src/input_windows.rs"]
mod input_windows;
#[allow(dead_code)]
#[path = "../../../src/shortcuts.rs"]
mod shortcuts;
#[allow(dead_code)]
#[path = "../../../src/wide.rs"]
mod wide;

mod command;
mod commands;
mod input_host;
mod native;
mod settings;
mod setup;
mod state;
mod surface;
mod tick;
mod voice;

pub use commands::*;
pub use setup::setup;
pub use state::{Gaming, bind, clear, forget};
pub use surface::{menu_items, open_probe_surfaces, open_settings};
