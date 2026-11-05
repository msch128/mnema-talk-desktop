//! The Tauri commands of the gaming surfaces and of the pages of servers. Each
//! one checks which window or webview is asking before it does anything.
use super::command::Command;
use super::game_candidate;
use super::settings::Settings;
use super::state::Gaming;
use super::tick::wake;
use super::voice::{Snapshot, Voice};
use crate::web_bundle::write_atomic;
use std::{sync::Mutex, time::Duration};
use tauri::{Manager, Webview, WebviewWindow};

fn local(window: &WebviewWindow) -> bool {
    window.url().is_ok_and(|u| crate::web_client::bundled(&u))
}
#[tauri::command]
pub fn desktop_gaming_sync(
    webview: Webview,
    state: tauri::State<'_, Mutex<Gaming>>,
    token: String,
    voice: Voice,
) -> Result<(), String> {
    let origin =
        crate::web_client::local_origin(&webview.url().map_err(|_| "Gaming unavailable.")?);
    let Some(server) = crate::instances::id_of_instance_label(webview.label()) else {
        return Err("Wrong gaming session.".into());
    };
    let result = {
        let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
        g.accept_sync(server, &token, origin.as_deref(), voice)
    };
    wake(webview.app_handle());
    result
}
#[tauri::command]
pub fn desktop_gaming_snapshot(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
) -> Result<Snapshot, String> {
    if !matches!(
        window.label(),
        "sidepeek" | "gaming-overlay" | "gaming-settings"
    ) || !local(&window)
    {
        return Err("Wrong gaming surface.".into());
    }
    let g = state.lock().map_err(|_| "Gaming unavailable.")?;
    let active = window.label() != "gaming-settings"
        && g.settings.enabled
        && g.active
        && g.updated.elapsed() < Duration::from_secs(2)
        && g.voice.connected;
    Ok(Snapshot {
        voice: if active {
            g.voice.clone()
        } else {
            Voice::default()
        },
        settings: g.settings.clone(),
        overlay: g.overlay && active,
        active,
        detection_note: g.detection_note,
        input_note: g.input_note,
        exclusive_fullscreen_hint: g.exclusive_hint,
    })
}
#[tauri::command]
pub fn desktop_gaming_control(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    action: String,
) -> Result<(), String> {
    if window.label() == "gaming-settings" && local(&window) && action == "close" {
        return window.close().map_err(|_| "Cannot close settings.".into());
    }
    if window.label() != "gaming-overlay" || !local(&window) {
        return Err("Wrong gaming surface.".into());
    }
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    if !g.active || !g.voice.connected || g.updated.elapsed() >= Duration::from_secs(2) {
        return Err("No current Talk connection.".into());
    }
    if action == "close" {
        g.overlay = false;
        return Ok(());
    }
    if !matches!(action.as_str(), "mute" | "deafen" | "stream" | "ptt-mode") {
        return Err("Unknown gaming control.".into());
    }
    let command = Command::new(&g, &action);
    if action == "stream" && !g.voice.sharing {
        g.overlay = false;
    }
    drop(g);
    command.dispatch(window.app_handle());
    Ok(())
}
/// Saves `settings` and makes them current. The command to release push-to-talk
/// is returned for the caller to send after it lets go of the lock.
fn apply_settings(
    app: &tauri::AppHandle,
    g: &mut Gaming,
    settings: Settings,
) -> Result<Command, String> {
    settings.validate()?;
    let dir = crate::storage::config_dir(app).ok_or("Settings unavailable.")?;
    std::fs::create_dir_all(&dir).map_err(|_| "Cannot save settings.")?;
    let data = serde_json::to_vec(&settings).map_err(|_| "Invalid settings.")?;
    write_atomic(&dir.join("gaming.json"), &data).map_err(|_| "Cannot save settings.")?;
    let command = Command::new(g, "ptt-release");
    g.ptt_down = false;
    g.ptt_armed = false;
    g.previous = [true; 4];
    if !settings.enabled {
        g.active = false;
        g.overlay = false;
    }
    g.settings = settings;
    g.sync_chords();
    g.library.rescan_now();
    Ok(command)
}
#[tauri::command]
pub fn desktop_gaming_settings(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    settings: Settings,
) -> Result<(), String> {
    if !matches!(window.label(), "gaming-overlay" | "gaming-settings") || !local(&window) {
        return Err("Wrong gaming surface.".into());
    }
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    let command = apply_settings(window.app_handle(), &mut g, settings)?;
    drop(g);
    command.dispatch(window.app_handle());
    Ok(())
}
/// The file name of the last other application that was in front, for the
/// person to confirm as a game. Asked for only by the settings or overlay on a
/// button press; it is the bare name, never a path, and it stays on the device.
#[tauri::command]
pub fn desktop_gaming_candidate(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
) -> Result<Option<String>, String> {
    if !matches!(window.label(), "gaming-overlay" | "gaming-settings") || !local(&window) {
        return Err("Wrong gaming surface.".into());
    }
    let g = state.lock().map_err(|_| "Gaming unavailable.")?;
    Ok(g.candidate.clone())
}
fn change_games(
    window: &WebviewWindow,
    state: &Mutex<Gaming>,
    change: impl FnOnce(&[String]) -> Result<Vec<String>, String>,
) -> Result<Vec<String>, String> {
    if !matches!(window.label(), "gaming-overlay" | "gaming-settings") || !local(window) {
        return Err("Wrong gaming surface.".into());
    }
    let mut g = state.lock().map_err(|_| "Gaming unavailable.")?;
    let mut settings = g.settings.clone();
    settings.games = change(&settings.games)?;
    let games = settings.games.clone();
    let command = apply_settings(window.app_handle(), &mut g, settings)?;
    drop(g);
    command.dispatch(window.app_handle());
    Ok(games)
}
/// Adds one game by executable file name (the confirmed candidate, or a typed
/// name) to the list kept on this device; returns the new list.
#[tauri::command]
pub fn desktop_gaming_add_game(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    name: String,
) -> Result<Vec<String>, String> {
    change_games(&window, &state, |games| {
        game_candidate::add_game(games, &name)
    })
}
/// Takes one game off the list; returns the new list.
#[tauri::command]
pub fn desktop_gaming_remove_game(
    window: WebviewWindow,
    state: tauri::State<'_, Mutex<Gaming>>,
    name: String,
) -> Result<Vec<String>, String> {
    change_games(&window, &state, |games| {
        Ok(game_candidate::remove_game(games, &name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_replace_the_file_whole_and_leave_no_temporary() {
        let dir = std::env::temp_dir().join(format!("mnema-gaming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("gaming.json");
        write_atomic(&file, b"{\"old\":1}").unwrap();
        write_atomic(&file, b"{\"new\":2}").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"{\"new\":2}");
        let mut partial = file.as_os_str().to_owned();
        partial.push(".partial");
        assert!(!std::path::PathBuf::from(partial).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
