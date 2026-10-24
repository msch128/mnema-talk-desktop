pub mod discovery;
pub mod foreground;
#[cfg(windows)]
pub mod foreground_windows;
pub mod game_identify;
pub mod game_library;
pub mod game_library_scan;
pub mod game_list;
pub mod game_owner;
pub mod gaming;
pub mod input;
#[cfg(windows)]
pub mod input_windows;
pub mod profiles;
pub mod shortcuts;
pub mod windows;

#[cfg(feature = "gaming-fixture")]
pub mod gaming_fixture;
#[cfg(feature = "gaming-fixture")]
pub mod native_input;
#[cfg(feature = "gaming-fixture")]
pub mod native_window_owner;

#[cfg(feature = "owned-d3d11-fixture")]
pub mod owned_d3d11;
