//! The update pill of the server rail: the rail asks for the state, the host
//! pushes every change, and a click is "Update now" or opens the window.
use super::now::request_now;
use super::state::{NativeUpdates, Snapshot};
use super::window::open_window;
use std::sync::Arc;
use tauri::{Manager, Webview};

/// The rail's state of the update. The rail also says which language it shows,
/// which the menu entries follow.
#[tauri::command]
pub fn rail_update_state(
    webview: Webview,
    state: tauri::State<'_, NativeUpdates>,
    german: bool,
) -> Result<Snapshot, String> {
    crate::web_shell::from_rail(&webview)?;
    state.set_german(german);
    Ok(state.snapshot())
}

/// `now`: the pill's "Update now". `open`: the update window (details, errors).
#[tauri::command]
pub fn rail_update_action(
    webview: Webview,
    state: tauri::State<'_, NativeUpdates>,
    action: String,
) -> Result<Snapshot, String> {
    let app = crate::web_shell::from_rail(&webview)?;
    match action.as_str() {
        "now" => {
            request_now(&app, false);
        }
        "open" => {
            open_window(&app, true).map_err(|_| "Cannot open the update window.")?;
        }
        _ => return Err("Unknown action.".into()),
    }
    Ok(state.snapshot())
}

/// Every change of the state reaches the rail page.
pub fn push_to_rail(app: &tauri::AppHandle) {
    let handle = app.clone();
    let push = Arc::new(move |snapshot: &Snapshot| {
        let Ok(json) = serde_json::to_string(snapshot) else {
            return;
        };
        if let Some(rail) = handle.get_webview(crate::web_shell::RAIL) {
            let _ = rail.eval(format!(
                "window.mnemaRail&&window.mnemaRail.update&&window.mnemaRail.update({json})"
            ));
        }
    });
    if let Some(state) = app.try_state::<NativeUpdates>()
        && let Ok(mut slot) = state.push.lock()
    {
        *slot = Some(push);
    }
}
