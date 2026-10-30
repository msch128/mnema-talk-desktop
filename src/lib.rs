pub mod foreground;
#[cfg(windows)]
pub mod foreground_windows;
pub mod fullscreen;
pub mod game_candidate;
pub mod game_identify;
pub mod game_library;
pub mod game_library_scan;
pub mod game_list;
#[cfg(windows)]
pub mod hung_window;
pub mod input;
#[cfg(windows)]
pub mod input_windows;
pub mod shortcuts;
pub mod ui_guard;
pub mod wide;
pub mod windows;
