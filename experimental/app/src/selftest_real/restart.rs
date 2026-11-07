//! The server stops and starts again on its port and database: the script does it, on request.
use super::{Target, page::*};
use std::time::Duration;

fn control_has(target: &Target, word: &str) -> bool {
    std::fs::read_to_string(&target.account.control)
        .is_ok_and(|text| text.lines().any(|line| line == word))
}

pub(super) async fn through(target: &Target<'_>) {
    record("restart");
    if !until("the server is stopped", 120, || {
        control_has(target, "stopped")
    })
    .await
    {
        return;
    }
    check(
        "the WebSocket ends when the server goes",
        target.event("e.kind==='close'").await,
        "no close event",
    );
    let down = target
        .page("return await fetch('/api/health').then((r)=>r.ok?'reached':'down',()=>'down')")
        .await;
    check("requests fail while it is down", down == "\"down\"", &down);
    record("down-checked");
    if !until("the server is back", 180, || control_has(target, "started")).await {
        return;
    }
    let mut again = String::new();
    for _ in 0..100 {
        again = target
            .page("return (await api('GET','/api/channels')).s")
            .await;
        if again == "200" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    check("the session survives the restart", again == "200", &again);
}
