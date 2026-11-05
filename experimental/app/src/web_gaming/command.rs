//! A command for a server's page (mute, deafen, push-to-talk, ...), built under
//! the lock and sent after it is let go.
use super::state::Gaming;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::Manager;

pub(super) struct Command {
    pub(super) created: Instant,
    pub(super) issued: f64,
    /// The server the command goes back to, and its page's token.
    pub(super) owner: Option<String>,
    pub(super) token: String,
    pub(super) origin: Option<String>,
    pub(super) scope: Option<String>,
    pub(super) action: String,
    pub(super) focus: bool,
}
impl Command {
    pub(super) fn new(g: &Gaming, action: &str) -> Self {
        Self {
            created: Instant::now(),
            issued: g.voice.clock + g.updated.elapsed().as_secs_f64() * 1000.0,
            owner: g.owner.clone(),
            token: g
                .owner
                .as_ref()
                .and_then(|o| g.tokens.get(o))
                .cloned()
                .unwrap_or_default(),
            origin: g.origin.clone(),
            scope: g.voice.scope.clone(),
            action: action.into(),
            focus: action == "stream" && !g.voice.sharing,
        }
    }
    pub(super) fn dispatch(self, app: &tauri::AppHandle) {
        let Self {
            created,
            issued,
            owner,
            token,
            origin,
            scope,
            action,
            focus,
        } = self;
        let detail = serde_json::json!({"token":token,"scope":scope,"action":action,"issued":issued,"expires":issued+150.0});
        let next = app.clone();
        let _ = app.run_on_main_thread(move || {
            let shared = next.state::<Mutex<Gaming>>();
            let Ok(current) = shared.lock() else {
                return;
            };
            let token_current = owner
                .as_ref()
                .and_then(|o| current.tokens.get(o))
                .is_some_and(|t| *t == token);
            if !token_current
                || current.owner != owner
                || current.origin != origin
                || (action != "ptt-release"
                    && (created.elapsed() >= Duration::from_millis(150)
                        || (action == "ptt-press" && !current.ptt_down)
                        || current.voice.scope != scope
                        || !current.active
                        || current.updated.elapsed() >= Duration::from_secs(2)))
            {
                return;
            }
            drop(current);
            let Some(server) = owner else {
                return;
            };
            if let Some(page) = next.get_webview(&crate::instances::instance_label(&server)) {
                if origin.is_none()
                    || !page
                        .url()
                        .is_ok_and(|u| crate::web_client::local_origin(&u) == origin)
                {
                    return;
                }
                let _ = page.eval(format!(
                "window.dispatchEvent(new CustomEvent('mnema-gaming-command',{{detail:{detail}}}));"
            ));
                if focus {
                    crate::web_shell::bring_forward(&next, &server);
                }
            }
        });
    }
}
