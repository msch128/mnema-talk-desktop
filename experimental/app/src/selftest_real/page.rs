//! The helpers of the self-test: the result record and the page probes.
use crate::selftest_page::run;
use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

pub(super) fn record(line: &str) {
    crate::web_permissions::append_log_line("MNEMA_REAL_SELFTEST", line);
}

/// How many checks ran, so the script compares the passes with the host's own count.
pub(super) static CHECKS: AtomicU32 = AtomicU32::new(0);

pub(super) fn check(name: &str, good: bool, detail: impl std::fmt::Display) {
    CHECKS.fetch_add(1, Ordering::Relaxed);
    if good {
        record(&format!("ok {name}"));
    } else {
        record(&format!("fail {name}: {detail}"));
    }
}

pub(super) async fn until(what: &str, seconds: u64, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..seconds * 10 {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    record(&format!("fail {what}: timed out"));
    false
}

/// What the page needs to talk to the server the way the web app does.
const API: &str = "const api=async(m,p,b)=>{const r=await fetch(p,{method:m,headers:{'Content-Type':'application/json'},body:b===undefined?undefined:JSON.stringify(b)});let j=null;try{j=await r.json()}catch(e){}return{s:r.status,j}};";

/// Opens `/api/ws` through the host and collects its events in the page.
pub(super) const SOCKET: &str = "window.__events=[];const ch=window.__TAURI_INTERNALS__.transformCallback((m)=>window.__events.push(m.message??m));return await window.__TAURI_INTERNALS__.invoke('desktop_ws_open',{path:'/api/ws',onEvent:'__CHANNEL__:'+ch});";

pub(super) async fn page(app: &tauri::AppHandle, label: &str, body: &str) -> String {
    run(app, label, &format!("{API}{body}")).await
}

/// Waits for the page to reach the host; reports its role and the origin it runs on.
pub(super) async fn bridge(app: &tauri::AppHandle, label: &str) -> String {
    for _ in 0..100 {
        let identity = run(
            app,
            label,
            "return [window.__MNEMA_TRUSTED_APP__.role,location.origin]",
        )
        .await;
        if identity.starts_with('[') {
            return identity;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    "no page".into()
}
