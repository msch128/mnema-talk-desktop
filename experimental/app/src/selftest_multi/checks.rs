//! The checks that run against the two servers.
use super::probe::*;
use crate::{
    instances::{Phase, instance_label},
    web_client::Trusted,
    web_shell,
    web_vault::{Os, Save, Vault},
};
use std::{sync::Arc, time::Duration};
use tauri::Manager;
use url::Url;

pub(super) async fn isolation(app: &tauri::AppHandle, page_a: &str, page_b: &str) {
    let identity = run(
        app,
        page_a,
        "const t=window.__MNEMA_TRUSTED_APP__;return [t.role,t.capabilities.includes('unread'),t.capabilities.includes('ws_query')]",
    )
    .await;
    check(
        "a page is told it is an instance with unread and ws_query",
        identity == "[\"instance\",true,true]",
        &identity,
    );
    // Browser storage: what A's page keeps, B's page does not see, and back.
    run(app, page_a, "localStorage.setItem('probe','A');return 1").await;
    let seen_by_b = run(app, page_b, "return localStorage.getItem('probe')").await;
    check(
        "storage of A is not visible to B",
        seen_by_b == "null",
        &seen_by_b,
    );
    run(app, page_b, "localStorage.setItem('probe','B');return 1").await;
    let seen_by_a = run(app, page_a, "return localStorage.getItem('probe')").await;
    check(
        "storage of B does not replace A's",
        seen_by_a == "\"A\"",
        &seen_by_a,
    );
    // Cookies live in the host: each server gets only its own.
    let echo = "const r=await fetch('/api/echo');return (await r.json()).cookie";
    run(
        app,
        page_a,
        "await fetch('/api/login',{method:'POST'});return 1",
    )
    .await;
    let a_cookie = run(app, page_a, echo).await;
    let b_cookie = run(app, page_b, echo).await;
    check(
        "A's page sends A's cookie",
        a_cookie == "\"session=A-token\"",
        &a_cookie,
    );
    check("B's page sends none of A's", b_cookie == "null", &b_cookie);
    run(
        app,
        page_b,
        "await fetch('/api/login',{method:'POST'});return 1",
    )
    .await;
    let b_cookie = run(app, page_b, echo).await;
    let a_cookie = run(app, page_a, echo).await;
    check(
        "each keeps its own cookie after both signed in",
        b_cookie == "\"session=B-token\"" && a_cookie == "\"session=A-token\"",
        format!("{a_cookie} {b_cookie}"),
    );
    // A page cannot drive the list, whoever it is.
    let refusal = run(
        app,
        page_a,
        "try{await window.__TAURI_INTERNALS__.invoke('rail_remove',{id:'0'});return 'allowed'}catch(e){return String(e)}",
    )
    .await;
    check(
        "a page cannot use the rail's commands",
        refusal.contains("not allowed") || refusal.contains("Not allowed"),
        &refusal,
    );
    let refusal = run(
        app,
        page_a,
        "try{await window.__TAURI_INTERNALS__.invoke('desktop_open_instance',{address:'http://127.0.0.1:9'});return 'allowed'}catch(e){return String(e)}",
    )
    .await;
    check(
        "a page cannot add servers",
        refusal.contains("not allowed") || refusal.contains("Not allowed"),
        &refusal,
    );
}

pub(super) async fn counters(app: &tauri::AppHandle, a: &str, b: &str, page_a: &str, page_b: &str) {
    web_shell::menu_clicked(app, "server-1");
    check(
        "Ctrl+1 brings the first server to the front",
        front(app).as_deref() == Some(a),
        format!("{:?}", front(app)),
    );
    let report = |u: u32, m: u32| {
        format!(
            "await window.__TAURI_INTERNALS__.invoke('desktop_report_unread',{{unread:{u},mentions:{m}}});return 1"
        )
    };
    run(app, page_b, &report(2, 3)).await;
    check(
        "B's counts show on B while A is in front",
        counts(app, b) == Some((2, 3)) && counts(app, a) == Some((0, 0)),
        format!("{:?} {:?}", counts(app, a), counts(app, b)),
    );
    check(
        "the icon badge is the sum",
        badge(app) == 3,
        format!("{}", badge(app)),
    );
    check(
        "a report does not switch servers",
        front(app).as_deref() == Some(a),
        format!("{:?}", front(app)),
    );
    run(app, page_a, &report(0, 100_000)).await;
    check(
        "counts are bounded",
        counts(app, a) == Some((0, 999)),
        format!("{:?}", counts(app, a)),
    );
    run(app, page_a, &report(0, 0)).await;
    run(
        app,
        page_a,
        "await window.__TAURI_INTERNALS__.invoke('desktop_set_badge',{count:4});return 1",
    )
    .await;
    check(
        "a page that reported does not fall back to the plain badge",
        counts(app, a) == Some((0, 0)),
        format!("{:?}", counts(app, a)),
    );
    web_shell::menu_clicked(app, "server-2");
    check(
        "Ctrl+2 switches back",
        front(app).as_deref() == Some(b),
        format!("{:?}", front(app)),
    );
    check(
        "the one in front keeps running and the other too",
        counts(app, b) == Some((2, 3)),
        format!("{:?}", counts(app, b)),
    );
}

fn sessions(
    trusted: &Trusted,
    a: &str,
    b: &str,
) -> (
    Option<Arc<crate::instances::Session>>,
    Option<Arc<crate::instances::Session>>,
) {
    let registry = trusted.registry.lock().unwrap();
    (registry.session(a), registry.session(b))
}

pub(super) async fn sockets(app: &tauri::AppHandle, trusted: &Trusted, a: &str, b: &str) {
    let sessions = sessions(trusted, a, b);
    let (Some(sa), Some(sb)) = sessions else {
        check("sessions", false, "missing");
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let id = crate::web_ws::open(&sa.sockets, Arc::clone(&sa.instance), true, move |event| {
        tx.send(event).is_ok()
    });
    check("A's socket opens", id.is_ok(), format!("{id:?}"));
    let opened = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await;
    check(
        "A's socket is accepted by A's server",
        matches!(opened, Ok(Some(crate::web_ws::WsEvent::Open { .. }))),
        format!("{opened:?}"),
    );
    if let Ok(id) = id {
        check(
            "B cannot send on A's socket",
            sb.sockets.send(id, "x".into()).is_err(),
            "",
        );
        check(
            "B cannot close A's socket",
            {
                sb.sockets.close(id);
                sa.sockets.send(id, "still open".into()).is_ok()
            },
            "",
        );
        sa.sockets.close(id);
    }
    let _ = app;
}

pub(super) async fn removal(
    app: &tauri::AppHandle,
    trusted: &Trusted,
    a: &str,
    b: &str,
    second: &str,
) {
    let origin_b = Url::parse(&format!("{second}/")).unwrap();
    let stored = trusted.jar_file(&origin_b);
    // A keychain that exists but refuses (a locked one on a headless runner)
    // keeps the session in memory on purpose: it never goes to a file.
    let probe = format!("{b}-probe");
    let refused = Os.save(&probe, b"probe") == Save::Refused;
    Os.remove(&probe);
    let had_session =
        Os.load(b).found().is_some() || stored.as_ref().is_some_and(|f| f.exists()) || refused;
    check(
        "B's session is stored",
        had_session,
        "neither keychain nor file",
    );
    let profile = trusted.data_dir().map(|d| d.join("profiles").join(b));
    web_shell::remove_server(app, b).await;
    let gone = until("B's page closes", || {
        app.get_webview(&instance_label(b)).is_none()
    })
    .await;
    check("B's page is gone", gone, "");
    check(
        "the list has only A, in front",
        trusted
            .registry
            .lock()
            .is_ok_and(|r| r.ids() == [a] && r.active() == Some(a)),
        "",
    );
    check(
        "B's stored session is wiped",
        Os.load(b).found().is_none() && !stored.as_ref().is_some_and(|f| f.exists()),
        "still stored",
    );
    check(
        "the badge follows",
        trusted.registry.lock().is_ok_and(|r| r.badge() == 0),
        "",
    );
    #[cfg(not(target_os = "macos"))]
    if let Some(dir) = profile {
        let wiped = until("B's storage is wiped", || !dir.exists()).await;
        check("B's browser storage is wiped", wiped, dir.display());
    }
    #[cfg(target_os = "macos")]
    {
        let _ = profile;
        let wiped = async {
            for _ in 0..50 {
                let ids = app.fetch_data_store_identifiers().await.unwrap_or_default();
                if ids.len() <= 2 {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            false
        }
        .await;
        check("B's browser storage is wiped", wiped, "data store remains");
    }
    let list = trusted
        .instances_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default();
    check(
        "the list on disk names only A",
        !list.contains(second) && list.contains(&a_text(trusted, a)),
        &list,
    );
    let a_kept = run(
        app,
        &instance_label(a),
        "return localStorage.getItem('probe')",
    )
    .await;
    check("A is untouched by the removal", a_kept == "\"A\"", &a_kept);
    let reload = matches!(
        trusted
            .registry
            .lock()
            .map(|r| r.slot(a).map(|s| matches!(s.phase, Phase::Ready(_)))),
        Ok(Some(true))
    );
    check("A keeps running", reload, "");
}

fn a_text(trusted: &Trusted, id: &str) -> String {
    trusted
        .registry
        .lock()
        .ok()
        .and_then(|r| r.slot(id).map(|s| s.origin.to_string()))
        .unwrap_or_default()
}

/// The storage the single-server client left is the engine's default one. It
/// must go even when no page is open, and on every kind of website data.
pub(super) async fn old_storage(app: &tauri::AppHandle) {
    let read = "return localStorage.getItem('old')";
    let seeded = async {
        let seed = web_shell::open_default_page(app, "legacyseed").await.ok()?;
        for _ in 0..100 {
            let wrote = run(
                app,
                "legacyseed",
                "localStorage.setItem('old','secret');await new Promise((r)=>{const o=indexedDB.open('old');o.onsuccess=()=>{o.result.close();r()};o.onerror=()=>r()});return localStorage.getItem('old')",
            )
            .await;
            if wrote == "\"secret\"" {
                let _ = seed.close();
                return Some(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        None
    }
    .await;
    check(
        "data can be left in the default storage",
        seeded.is_some(),
        "",
    );
    let probe = web_shell::open_default_page(app, "legacyprobe").await;
    let before = if probe.is_ok() {
        run_when_ready(app, "legacyprobe", read).await
    } else {
        "no page".into()
    };
    check(
        "a new page on the default storage sees it",
        before == "\"secret\"",
        &before,
    );
    if let Ok(probe) = probe {
        let _ = probe.close();
    }
    web_shell::wipe_default_storage(app).await;
    let after_page = web_shell::open_default_page(app, "legacyafter").await;
    let after = if after_page.is_ok() {
        run_when_ready(app, "legacyafter", read).await
    } else {
        "no page".into()
    };
    check(
        "the default storage is empty after the wipe",
        after == "null",
        &after,
    );
    if let Ok(page) = after_page {
        let _ = page.close();
    }
}
