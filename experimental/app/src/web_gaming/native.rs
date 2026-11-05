//! What the operating system says about the window in front: whether it is a
//! game, and where the sidepeek goes. Only Windows has an answer.
use super::state::Gaming;
#[cfg(any(windows, test))]
use super::{foreground, game_identify, game_library, game_list};
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use tauri::Manager;

/// The game window being tracked. It holds no handle to the game's process: the
/// window handle alone says whether it is still the same window of the same
/// process, so nothing is ever opened on a game for this (anti-cheat safety).
#[cfg(windows)]
pub(super) struct GameLease {
    hwnd: usize,
    pid: u32,
}
#[cfg(windows)]
impl GameLease {
    fn new(hwnd: usize, pid: u32) -> Self {
        Self { hwnd, pid }
    }
    fn live(&self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
        };
        let mut pid = 0;
        let hwnd = self.hwnd as *mut std::ffi::c_void;
        unsafe {
            IsWindow(hwnd) != 0
                && IsWindowVisible(hwnd) != 0
                && IsIconic(hwnd) == 0
                && GetWindowThreadProcessId(hwnd, &mut pid) != 0
                && pid == self.pid
        }
    }
}
#[cfg(windows)]
pub(super) fn native_sample(
    g: &mut Gaming,
    app: &tauri::AppHandle,
) -> (bool, Option<tauri::PhysicalPosition<i32>>) {
    use windows_sys::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        },
    };
    let hwnd = unsafe { GetForegroundWindow() };
    let own_overlay = app
        .get_webview_window("gaming-overlay")
        .and_then(|w| w.hwnd().ok())
        .is_some_and(|h| h.0 == hwnd);
    // The foreground source announced this window; a newer foreground that has
    // not been announced yet simply does not match, so a stale game is never shown.
    let library = g.library.current();
    let identified = announced_game(
        g.foreground.as_ref(),
        hwnd as usize,
        &g.settings.games,
        &library,
        || window_title(hwnd),
    );
    let detected = identified.is_some();
    if let Some(f) = g.foreground.clone().filter(|_| detected) {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != f.pid {
            return (false, None);
        }
        if !g
            .game
            .as_ref()
            .is_some_and(|game| game.hwnd == hwnd as usize && game.pid == pid && game.live())
        {
            g.game = Some(GameLease::new(hwnd as usize, pid));
        }
    }
    let game_live = g.game.as_ref().is_some_and(GameLease::live);
    let active = game_live && (detected || (g.overlay && g.active && own_overlay));
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 || unsafe { IsIconic(hwnd) } != 0 {
        return (false, None);
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let position = if detected && unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        Some(tauri::PhysicalPosition::new(
            info.rcMonitor.left + 12,
            info.rcMonitor.top + (info.rcMonitor.bottom - info.rcMonitor.top) / 4,
        ))
    } else {
        None
    };
    (active && hwnd == unsafe { GetForegroundWindow() }, position)
}
/// The announced foreground when it is the window in front now and a game the
/// user's list, a launcher library or the bundled list names. `title` is asked
/// only for Java runtimes, which are a Minecraft candidate by their title alone.
#[cfg(any(windows, test))]
fn announced_game(
    front: Option<&foreground::Foreground>,
    window: usize,
    names: &[String],
    library: &game_library::InstallIndex,
    title: impl FnOnce() -> String,
) -> Option<game_identify::Identified> {
    let front = front.filter(|f| window != 0 && f.window == window)?;
    let title = if game_identify::is_java(front.basename()) {
        title()
    } else {
        String::new()
    };
    game_identify::identify(
        names,
        library,
        game_list::GameList::bundled(),
        &front.image,
        &title,
    )
}
/// The title of a window, read only for Java runtimes (a shared runtime is a
/// Minecraft candidate only by its title). The window belongs to another
/// program, so only the caption the system keeps is read; nothing is sent to it.
#[cfg(windows)]
fn window_title(hwnd: *mut std::ffi::c_void) -> String {
    crate::ui_watch::hung_window::title(hwnd as usize, TITLE_TIMEOUT).unwrap_or_default()
}
#[cfg(windows)]
const TITLE_TIMEOUT: Duration = Duration::from_millis(50);
#[cfg(not(windows))]
pub(super) fn native_sample(
    _g: &mut Gaming,
    _app: &tauri::AppHandle,
) -> (bool, Option<tauri::PhysicalPosition<i32>>) {
    (false, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front(window: usize, image: &str) -> foreground::Foreground {
        foreground::Foreground {
            window,
            pid: 4,
            image: image.into(),
        }
    }
    #[test]
    fn only_the_announced_window_in_front_can_be_a_game() {
        let mine = vec!["Custom.exe".to_string()];
        let none = || String::new();
        let empty = game_library::InstallIndex::default();
        let game = front(5, r"D:\Games\Custom.exe");
        assert!(announced_game(Some(&game), 5, &mine, &empty, none).is_some());
        // A different window is in front now: the announcement is stale.
        assert!(announced_game(Some(&game), 6, &mine, &empty, none).is_none());
        assert!(announced_game(Some(&game), 0, &mine, &empty, none).is_none());
        assert!(announced_game(None, 5, &mine, &empty, none).is_none());
        // The system refused the image path: never a game.
        assert!(announced_game(Some(&front(5, "")), 5, &mine, &empty, none).is_none());
        let notepad = front(5, r"C:\Windows\notepad.exe");
        assert!(announced_game(Some(&notepad), 5, &mine, &empty, none).is_none());
    }
    #[test]
    fn launcher_library_games_are_named_by_their_launcher() {
        let library = game_library::InstallIndex::new(vec![
            game_library::InstalledGame::new(
                "Example Quest",
                r"D:\SteamLibrary\steamapps\common\Example Quest",
            )
            .unwrap(),
        ]);
        let inside = front(3, r"D:\SteamLibrary\steamapps\common\Example Quest\eq.exe");
        let found = announced_game(Some(&inside), 3, &[], &library, String::new).unwrap();
        assert_eq!(found.name, "Example Quest");
        let outside = front(3, r"D:\Other\eq.exe");
        assert!(announced_game(Some(&outside), 3, &[], &library, String::new).is_none());
    }
    #[test]
    fn java_needs_a_minecraft_title_and_other_apps_never_ask_for_one() {
        let empty = game_library::InstallIndex::default();
        let java = front(1, r"C:\Java\bin\javaw.exe");
        let asked = std::cell::Cell::new(0);
        let minecraft = || {
            asked.set(asked.get() + 1);
            "Minecraft 1.21".to_string()
        };
        assert!(announced_game(Some(&java), 1, &[], &empty, minecraft).is_some());
        assert_eq!(asked.get(), 1);
        assert!(announced_game(Some(&java), 1, &[], &empty, || "Editor".into()).is_none());
        let other = front(2, r"D:\Games\Custom.exe");
        let never = || panic!("title read for a non-Java process");
        assert!(announced_game(Some(&other), 2, &["Custom.exe".into()], &empty, never).is_some());
        let helper = front(3, r"C:\Java\bin\javaws.exe");
        let never = || panic!("title read for a process that is not a Java runtime");
        assert!(announced_game(Some(&helper), 3, &[], &empty, never).is_none());
    }
}
