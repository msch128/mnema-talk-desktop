//! Test-only: runs the servers side by side against two stand-in instances
//! and records what it saw.
//!
//! Compiled only with the `multi-selftest` feature, which no shipped build
//! enables, and even then inert unless `MNEMA_MULTI_SELFTEST` names the file to
//! append result lines to and `MNEMA_MULTI_SELFTEST_SERVERS` holds the two
//! addresses (`scripts/multi-instance-selftest.mjs` starts them and reads the
//! record). It drives the real window and the real webviews: it adds both
//! servers, checks that storage, cookies, counters and sockets of one are not
//! those of the other, switches, and removes one. The line format (`ok <name>`,
//! `fail <name>: <why>`, `total <checks run>`, `done`) is the contract with that script.
mod checks;
mod probe;

use crate::{
    instances::{id_for, instance_label},
    web_client::Trusted,
    web_shell,
};
use checks::*;
use probe::*;
use std::{sync::atomic::Ordering, time::Duration};
use tauri::Manager;
use url::Url;

pub fn start(app: &tauri::AppHandle) {
    let (Some(_), Ok(servers)) = (
        std::env::var_os("MNEMA_MULTI_SELFTEST"),
        std::env::var("MNEMA_MULTI_SELFTEST_SERVERS"),
    ) else {
        return;
    };
    let addresses: Vec<String> = servers.split(',').map(str::to_owned).collect();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if addresses.len() == 2 {
            drive(&app, &addresses[0], &addresses[1]).await;
        } else {
            record("fail setup: two addresses are needed");
        }
        record(&format!("total {}", CHECKS.load(Ordering::Relaxed)));
        record("done");
        app.exit(0);
    });
}

/// Waits (up to ten seconds) until the page can reach the host.
async fn wait_for_bridge(app: &tauri::AppHandle, label: &str) {
    for _ in 0..100 {
        if eval(app, label, "typeof window.__TAURI_INTERNALS__")
            .await
            .is_some_and(|v| v == "\"object\"")
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn drive(app: &tauri::AppHandle, first: &str, second: &str) {
    let trusted = app.state::<Trusted>();
    let (a, b) = (
        id_for(&Url::parse(&format!("{first}/")).unwrap()),
        id_for(&Url::parse(&format!("{second}/")).unwrap()),
    );
    let (page_a, page_b) = (instance_label(&a), instance_label(&b));
    if !until("window", || {
        app.get_webview("rail").is_some() && app.get_webview("main").is_some()
    })
    .await
    {
        return;
    }
    let added = [
        web_shell::add_server(app, &trusted, first).await,
        web_shell::add_server(app, &trusted, second).await,
    ];
    check(
        "add two servers",
        added.iter().all(Result::is_ok),
        format!("{added:?}"),
    );
    check(
        "the last added is in front",
        front(app).as_deref() == Some(&b),
        format!("{:?}", front(app)),
    );
    check(
        "adding the same server again is not a second slot",
        web_shell::add_server(app, &trusted, first).await.is_ok()
            && trusted.registry.lock().is_ok_and(|r| r.len() == 2),
        "",
    );
    if !until("pages", || {
        app.get_webview(&page_a).is_some() && app.get_webview(&page_b).is_some()
    })
    .await
    {
        return;
    }
    for label in [&page_a, &page_b] {
        wait_for_bridge(app, label).await;
    }
    isolation(app, &page_a, &page_b).await;
    counters(app, &a, &b, &page_a, &page_b).await;
    sockets(app, &trusted, &a, &b).await;
    removal(app, &trusted, &a, &b, second).await;
    old_storage(app).await;
}
