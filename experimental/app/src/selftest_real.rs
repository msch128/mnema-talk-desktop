//! Test-only: runs the host against a real mnema-talk server and records what happened.
//!
//! Compiled only with the `real-selftest` feature, which no shipped build
//! enables, and even then inert unless `MNEMA_REAL_SELFTEST` names the file to
//! append result lines to. `MNEMA_REAL_SELFTEST_SERVER` is the address as a
//! person types it, `_USER` and `_PASSWORD` an account on that server and
//! `_CONTROL` a file the script appends to (`scripts/real-server-selftest.mjs`
//! starts the server, restarts it on request and reads the record). The host
//! adds the server, loads a bundle for it, signs in, opens the WebSocket, sends
//! a message and sees it arrive, saves the data export through the page's
//! download link, then follows the server through a restart.
//! The line format (`ok <name>`, `fail <name>: <why>`, `total <checks run>`,
//! `done`, and the requests `restart` and `down-checked`) is the contract with that script.
mod export;
mod page;
mod restart;

use crate::{
    instances::{Phase, id_for, instance_label},
    web_client::{Trusted, address},
    web_shell,
};
use page::*;
use std::sync::atomic::Ordering;
use tauri::Manager;
use url::Url;

pub struct Account {
    server: String,
    user: String,
    password: String,
    control: std::path::PathBuf,
}

/// The page of the server under test and the channel its messages go to.
struct Target<'a> {
    app: &'a tauri::AppHandle,
    label: String,
    channel: String,
    account: &'a Account,
}

pub fn start(app: &tauri::AppHandle) {
    let var = |name: &str| std::env::var(format!("MNEMA_REAL_SELFTEST_{name}")).ok();
    let (Some(_), Some(server), Some(user), Some(password), Some(control)) = (
        std::env::var_os("MNEMA_REAL_SELFTEST"),
        var("SERVER"),
        var("USER"),
        var("PASSWORD"),
        var("CONTROL"),
    ) else {
        return;
    };
    let account = Account {
        server,
        user,
        password,
        control: control.into(),
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        drive(&app, &account).await;
        record(&format!("total {}", CHECKS.load(Ordering::Relaxed)));
        record("done");
        app.exit(0);
    });
}

async fn drive(app: &tauri::AppHandle, account: &Account) {
    if !until("window", 180, || {
        app.get_webview("rail").is_some() && app.get_webview("main").is_some()
    })
    .await
    {
        return;
    }
    let Some(label) = open_server(app, account).await else {
        return;
    };
    let Some(target) = sign_in(app, account, label).await else {
        return;
    };
    target.chat("first").await;
    target.export().await;
    restart::through(&target).await;
    target.chat("second").await;
    target.history_and_sign_out().await;
}

/// Adds the server by the address a person types and waits for its page.
async fn open_server(app: &tauri::AppHandle, account: &Account) -> Option<String> {
    let trusted = app.state::<Trusted>();
    // A person who types `host:port` gets https; a server that speaks plain
    // http on this machine is typed with its scheme.
    let typed = account.server.trim_start_matches("http://");
    let bare = address(typed).map(|u| u.to_string());
    check(
        "a bare host:port address means https",
        bare.as_deref() == Ok(&format!("https://{typed}/")),
        format!("{bare:?}"),
    );
    let origin = address(&account.server).ok()?;
    let id = id_for(&Url::parse(origin.as_str()).ok()?);
    let added = web_shell::add_server(app, &trusted, &account.server).await;
    check(
        "the server is added by its typed address",
        added.is_ok(),
        format!("{added:?}"),
    );
    added.ok()?;
    let session = trusted
        .registry
        .lock()
        .ok()
        .and_then(|r| match r.slot(&id).map(|s| &s.phase) {
            Some(Phase::Ready(session)) => Some(session.clone()),
            _ => None,
        });
    check("the slot is ready", session.is_some(), "no running session");
    // A ready slot means the host verified a bundle with its pinned key and chose it by the server's own /api/health.
    let session = session?;
    record(&format!(
        "info bundle {} from {:?}",
        session.held.bundle.version, session.held.source
    ));
    let label = instance_label(&id);
    until("page", 60, || app.get_webview(&label).is_some())
        .await
        .then_some(label)
}

/// The page reaches the host, the server answers, and the account signs in.
async fn sign_in<'a>(
    app: &'a tauri::AppHandle,
    account: &'a Account,
    label: String,
) -> Option<Target<'a>> {
    let identity = bridge(app, &label).await;
    check(
        "the page of the bundle runs as an instance and reaches the host",
        crate::web_client::local_origin(&crate::web_client::app_url())
            .is_some_and(|app| identity == format!("[\"instance\",\"{app}\"]")),
        &identity,
    );
    let health = page(
        app,
        &label,
        "const r=await api('GET','/api/health');return [r.s,r.j&&r.j.status,r.j&&r.j.web_client_api]",
    )
    .await;
    check(
        "the server answers health through the host",
        health.starts_with("[200,\"ok\","),
        &health,
    );
    let anonymous = page(app, &label, "return (await api('GET','/api/channels')).s").await;
    check("channels need a session", anonymous == "401", &anonymous);
    let body = format!(
        "return (await api('POST','/api/auth/login',{{username:{:?},password:{:?}}})).s",
        account.user, account.password
    );
    let login = page(app, &label, &body).await;
    check("sign-in is accepted", login == "200", &login);
    let channel = page(
        app,
        &label,
        "const r=await api('GET','/api/channels');const c=r.j&&r.j.categories.flatMap((k)=>k.channels).find((c)=>c.type==='text');return c?c.id:null",
    )
    .await
    .trim_matches('"')
    .to_owned();
    check("the session lists channels", channel.len() == 36, &channel);
    (channel.len() == 36).then_some(Target {
        app,
        label,
        channel,
        account,
    })
}

impl Target<'_> {
    async fn page(&self, body: &str) -> String {
        page(self.app, &self.label, body).await
    }

    /// Opens a socket, sends a message with a text of its own and waits for the server to push it.
    async fn chat(&self, name: &str) {
        let socket = self.page(SOCKET).await;
        check(
            &format!("[{name}] the host opens the WebSocket"),
            socket.parse::<u64>().is_ok(),
            &socket,
        );
        check(
            &format!("[{name}] the server accepts the WebSocket"),
            self.event("e.kind==='open'").await,
            "no open event",
        );
        let text = format!("selftest-{name}-{}", uuid::Uuid::new_v4().simple());
        let status = self
            .page(&format!(
                "return (await api('POST','/api/channels/{}/messages',{{content:{text:?},nonce:crypto.randomUUID()}})).s",
                self.channel
            ))
            .await;
        check(
            &format!("[{name}] a message is sent"),
            status == "201" || status == "200",
            &status,
        );
        check(
            &format!("[{name}] the message arrives on the WebSocket"),
            self.event(&format!(
                "e.kind==='message'&&String(e.data).includes({text:?})"
            ))
            .await,
            "not pushed",
        );
    }

    /// Waits for an event of the page's socket that `test` (a JS expression over `e`) accepts.
    async fn event(&self, test: &str) -> bool {
        for _ in 0..200 {
            let seen = self
                .page(&format!("return window.__events.some((e)=>{test})"))
                .await;
            if seen == "true" {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    }

    async fn history_and_sign_out(&self) {
        let history = self
            .page(&format!(
                "const r=await api('GET','/api/channels/{}/messages');return JSON.stringify(r.j).includes('selftest-first-')",
                self.channel
            ))
            .await;
        check(
            "the message from before the restart is still stored",
            history == "true",
            &history,
        );
        let out = self
            .page("return (await api('POST','/api/auth/logout')).s")
            .await;
        check("sign-out is accepted", out == "204" || out == "200", &out);
    }
}
