//! The commands of the rail and of the page for adding a server. Each one
//! checks which webview is asking before it does anything.
use super::layout::focus_front;
use super::rail::{changed, counts_changed, rail_snapshot};
use super::room::room_for;
use super::servers::{add_server, remove_server, spawn_start, start_server};
use super::{ADD, RAIL};
use crate::instances::{RailState, instance_label};
use crate::web_bundle::is_hex_id16;
use crate::web_client::{Trusted, bundled};
use crate::web_proxy::{forward, now};
use tauri::{Manager, Webview};

pub(super) fn local_page(webview: &Webview, label: &str) -> bool {
    webview.label() == label && webview.url().is_ok_and(|u| bundled(&u))
}

/// The page for adding a server asks to open one.
#[tauri::command]
pub async fn desktop_open_instance(
    webview: Webview,
    state: tauri::State<'_, Trusted>,
    address: String,
) -> Result<(), String> {
    if !local_page(&webview, ADD) {
        return Err(crate::web_client::SELECTOR_ONLY.into());
    }
    add_server(webview.app_handle(), &state, &address).await
}

/// Only the rail may drive the list.
pub(crate) fn from_rail(webview: &Webview) -> Result<tauri::AppHandle, String> {
    if local_page(webview, RAIL) {
        Ok(webview.app_handle().clone())
    } else {
        Err("Only the server rail may use this.".into())
    }
}

#[tauri::command]
pub fn rail_state(webview: Webview) -> Result<RailState, String> {
    Ok(rail_snapshot(&from_rail(&webview)?))
}

#[tauri::command]
pub fn rail_activate(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    let found = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .activate(&id);
    if found {
        changed(&app);
        focus_front(&app);
    }
    Ok(())
}

pub(super) fn start_adding(app: &tauri::AppHandle) {
    let started = app.state::<Trusted>().registry.lock().is_ok_and(|mut r| {
        let room = room_for(r.len());
        if room {
            r.start_adding();
        }
        room
    });
    if started {
        changed(app);
        focus_front(app);
    }
}

#[tauri::command]
pub fn rail_add(webview: Webview) -> Result<(), String> {
    start_adding(&from_rail(&webview)?);
    Ok(())
}

#[tauri::command]
pub fn rail_cancel_add(webview: Webview) -> Result<(), String> {
    let app = from_rail(&webview)?;
    app.state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .stop_adding();
    changed(&app);
    focus_front(&app);
    Ok(())
}

/// The rail changes how it is laid out: order, folders, a server's local name.
/// See `rail_layout::apply` for what is asked and what is refused.
#[tauri::command]
// One argument per field the rail page sends; Tauri fills them by name.
#[allow(clippy::too_many_arguments)]
pub fn rail_edit(
    webview: Webview,
    op: String,
    id: Option<String>,
    target: Option<String>,
    before: Option<String>,
    delta: Option<i32>,
    flag: Option<bool>,
    name: Option<String>,
    color: Option<String>,
    until: Option<i64>,
    text: Option<String>,
) -> Result<serde_json::Value, String> {
    let app = from_rail(&webview)?;
    match op.as_str() {
        "mute" => return mute(&app, id.as_deref(), until).map(|()| serde_json::Value::Null),
        "mark_read" => return mark_read(&app, id.as_deref()).map(|()| serde_json::Value::Null),
        "export" => return export_list(&app).map(serde_json::Value::from),
        "import" => return import_list(&app, text.as_deref()).map(serde_json::Value::from),
        // `until`: the seconds hidden after which a page is closed, -1 for never.
        "unload_after" => return unload_after(&app, until).map(|()| serde_json::Value::Null),
        _ => {}
    }
    let edit = crate::rail_layout::Edit {
        op,
        id,
        target,
        before,
        delta,
        flag,
        name,
        color,
    };
    let mut outcome = Ok(());
    let done = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .edit(|layout| {
            outcome = crate::rail_layout::apply(layout, &edit, crate::rail_layout::new_folder_id);
            outcome.is_ok()
        });
    if done {
        changed(&app);
    }
    outcome.map(|()| serde_json::Value::Null)
}

fn server_id(id: Option<&str>) -> Result<&str, String> {
    id.filter(|id| is_hex_id16(id))
        .ok_or_else(|| "invalid arguments".to_owned())
}

/// Notifications of a server off until a time, for good, or on again.
fn mute(app: &tauri::AppHandle, id: Option<&str>, until: Option<i64>) -> Result<(), String> {
    let id = server_id(id)?;
    let until = until.ok_or("invalid arguments")?;
    let done = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .mute(id, until, now());
    if !done {
        return Err("The change was refused.".into());
    }
    changed(app);
    // The number on the icon leaves out a muted server.
    counts_changed(app);
    Ok(())
}

/// What the web app is told to do when the person marks a server as read.
const MARK_READ: &str = "window.dispatchEvent(new CustomEvent('mnema-desktop:mark-all-read'))";

/// Asks the page of a running server to mark everything read. Its answer is
/// the unread report it already sends.
fn mark_read(app: &tauri::AppHandle, id: Option<&str>) -> Result<(), String> {
    let id = server_id(id)?;
    let ready = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(id)
        .is_some();
    let page = app.get_webview(&instance_label(id));
    match page {
        Some(page) if ready => page
            .eval(MARK_READ)
            .map_err(|_| "The page is not available.".to_owned()),
        _ => Err("The server is not ready.".into()),
    }
}

/// The rail menu's "Unload idle servers": Off (-1), 600 or 3600 seconds.
fn unload_after(app: &tauri::AppHandle, until: Option<i64>) -> Result<(), String> {
    let after = match until {
        Some(-1) => None,
        Some(secs) => Some(u64::try_from(secs).map_err(|_| "Unknown choice.")?),
        None => return Err("Unknown choice.".into()),
    };
    let set = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .set_unload_after(after);
    if !set {
        return Err("Unknown choice.".into());
    }
    changed(app);
    Ok(())
}

/// Writes the list of servers to the Downloads folder (else the data folder)
/// and gives the path.
fn export_list(app: &tauri::AppHandle) -> Result<String, String> {
    let trusted = app.state::<Trusted>();
    let bytes = trusted
        .registry
        .lock()
        .map_err(|_| "busy")?
        .exported()
        .to_bytes()
        .map_err(|_| "The list could not be written.")?;
    let dir = app
        .path()
        .download_dir()
        .ok()
        .or_else(|| trusted.data_dir().map(std::path::Path::to_path_buf))
        .ok_or("There is no folder to write to.")?;
    crate::rail_list::write_export(&dir, &bytes)
        .map(|path| path.display().to_string())
        .map_err(|_| "The list could not be written.".to_owned())
}

/// Takes a list of servers in (see `Registry::merge`) and starts the new ones.
/// Gives how many servers were added.
fn import_list(app: &tauri::AppHandle, text: Option<&str>) -> Result<usize, String> {
    let stored = text
        .and_then(|text| crate::instances::Stored::parse(text.as_bytes()))
        .ok_or("The list could not be read.")?;
    let added = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .merge(&stored, now());
    changed(app);
    counts_changed(app);
    for id in &added {
        spawn_start(app, id.clone());
    }
    Ok(added.len())
}

#[tauri::command]
pub async fn rail_remove(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    remove_server(&app, &id).await;
    Ok(())
}

#[tauri::command]
pub async fn rail_reconnect(webview: Webview, id: String) -> Result<(), String> {
    let app = from_rail(&webview)?;
    let refused = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id)
        .is_none();
    if refused {
        start_server(&app, &id).await;
    }
    Ok(())
}

/// The signed-in member's server-wide notification level on one server, for the
/// rail's menu. Unsupported (an older server, or one not ready) is an answer,
/// not an error, so the menu simply leaves the entries out.
#[tauri::command]
pub async fn rail_notify_get(
    webview: Webview,
    id: String,
) -> Result<crate::rail_notify::NotifyState, String> {
    use crate::rail_notify::{HEALTH_PATH, NotifyState, SETTINGS_PATH, parse_settings, supports};
    let app = from_rail(&webview)?;
    let session = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id);
    let Some(session) = session else {
        return Ok(NotifyState::unsupported());
    };
    let ask = |path: &'static str| {
        let session = session.clone();
        async move {
            let resp = forward(&session.instance, "GET", path, &[], Vec::new()).await;
            // The health answer lists the features in its 503 as well (a dependency such as object storage is down).
            let answered = resp.status == 200 || (resp.status == 503 && path == HEALTH_PATH);
            answered.then_some(resp.body)
        }
    };
    let supported = ask(HEALTH_PATH).await.is_some_and(|body| supports(&body));
    if !supported {
        return Ok(NotifyState::unsupported());
    }
    Ok(ask(SETTINGS_PATH)
        .await
        .and_then(|body| parse_settings(&body))
        .unwrap_or_else(NotifyState::unsupported))
}

/// Sets that level. `suppress_everyone` is read from the server first and kept,
/// so the rail never changes more than the level.
#[tauri::command]
pub async fn rail_notify_set(webview: Webview, id: String, level: String) -> Result<(), String> {
    use crate::rail_notify::{SERVER_PATH, put_body};
    let app = from_rail(&webview)?;
    let current = rail_notify_get(webview, id.clone()).await?;
    if !current.supported {
        return Err("This server cannot set a notification level.".into());
    }
    let body = put_body(&level, current.suppress_everyone).ok_or("Unknown notification level.")?;
    let session = app
        .state::<Trusted>()
        .registry
        .lock()
        .map_err(|_| "busy")?
        .session(&id)
        .ok_or("This server is not connected.")?;
    let headers = [("content-type".to_owned(), "application/json".to_owned())];
    let resp = forward(&session.instance, "PUT", SERVER_PATH, &headers, body).await;
    if resp.status == 204 || resp.status == 200 {
        Ok(())
    } else {
        Err("The server did not accept the change.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Who may call what is decided by the capability files: the rail drives
    /// the list and nothing else; a server's page reaches its own socket,
    /// toasts, counters and gaming bridge and can neither drive the list nor
    /// add a server; only the page for adding may do that.
    fn capabilities() -> Vec<Value> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                serde_json::from_str(&text).unwrap()
            })
            .collect()
    }

    fn permissions(capability: &Value) -> Vec<&str> {
        capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect()
    }

    fn named<'a>(capabilities: &'a [Value], id: &str) -> &'a Value {
        capabilities
            .iter()
            .find(|c| c["identifier"] == id)
            .unwrap_or_else(|| panic!("no capability {id}"))
    }

    #[test]
    fn the_rail_drives_the_list_and_nothing_else() {
        let all = capabilities();
        let rail = named(&all, "server-rail");
        assert_eq!(rail["webviews"], json!(["rail"]));
        assert!(rail.get("windows").is_none());
        let permissions = permissions(rail);
        assert!(permissions.iter().all(|p| p.starts_with("allow-rail-")));
        assert_eq!(permissions.len(), 12);
        for command in ["edit", "menu", "dialog", "icon"] {
            assert!(permissions.contains(&format!("allow-rail-{command}").as_str()));
        }
        assert!(!permissions.contains(&"allow-rail-move"));
    }

    #[test]
    fn the_dialog_can_hand_back_its_answer_and_nothing_else() {
        // It is reached by its webview alone.
        let all = capabilities();
        let dialog = named(&all, "rail-dialog");
        assert_eq!(dialog["webviews"], json!(["rail-dialog"]));
        assert!(dialog.get("windows").is_none());
        assert_eq!(permissions(dialog), ["allow-dialog-done"]);
    }

    #[test]
    fn no_other_page_drives_the_list_answers_the_dialog_or_adds_a_server() {
        for capability in capabilities() {
            let id = capability["identifier"].as_str().unwrap();
            if id == "server-rail" || id == "rail-dialog" {
                continue;
            }
            let permissions = permissions(&capability);
            let names = |prefix: &str| permissions.iter().any(|p| p.starts_with(prefix));
            assert!(!names("allow-rail-"), "{id} may drive the list");
            assert!(!names("allow-dialog-"), "{id} may answer the dialog");
            if capability["webviews"] == json!(["i-*"]) {
                assert!(
                    !names("allow-desktop-open-instance"),
                    "{id}: a page of a server may not add servers"
                );
                assert!(
                    capability.get("windows").is_none(),
                    "{id}: by webview, not window"
                );
            }
        }
    }

    #[test]
    fn every_capability_file_is_in_force() {
        // tauri.conf.json names each one.
        let conf: Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let in_force: Vec<&str> = conf["app"]["security"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for capability in capabilities() {
            assert!(in_force.contains(&capability["identifier"].as_str().unwrap()));
        }
    }

    #[test]
    fn marking_a_server_read_dispatches_the_event_the_web_app_listens_for() {
        assert!(MARK_READ.contains("mnema-desktop:mark-all-read"));
    }
}
