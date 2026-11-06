//! Test-only: runs script in the pages of the host's webviews and returns what it produced.
//!
//! Shared by the self-tests (`multi-selftest`, `real-selftest`), which no shipped build enables.
use std::time::Duration;
use tauri::Manager;
use tokio::sync::oneshot;

/// The value of an expression in the page of a webview, as JSON text.
pub(crate) async fn eval(app: &tauri::AppHandle, label: &str, script: &str) -> Option<String> {
    let page = app.get_webview(label)?;
    let (tx, rx) = oneshot::channel();
    let tx = std::sync::Mutex::new(Some(tx));
    page.eval_with_callback(script, move |value| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(value);
        }
    })
    .ok()?;
    tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .ok()?
        .ok()
}

/// Runs an async body in the page and waits for its (JSON) result.
pub(crate) async fn run(app: &tauri::AppHandle, label: &str, body: &str) -> String {
    let start = format!(
        "window.__r=undefined;(async()=>{{try{{window.__r=JSON.stringify(await (async()=>{{{body}}})())}}catch(e){{window.__r='ERR '+String(e)}}}})();1"
    );
    if eval(app, label, &start).await.is_none() {
        return "no page".into();
    }
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(value) = eval(app, label, "window.__r").await
            && let Ok(Some(text)) = serde_json::from_str::<Option<String>>(&value)
        {
            return text;
        }
    }
    "timeout".into()
}
