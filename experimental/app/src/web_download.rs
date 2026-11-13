//! Downloads of a server's page. A download link of the app's own API is
//! handed to the host by a page shim (`page-shim/app-downloads.js`): the host
//! streams the file into the Downloads folder (`web_proxy::download_to`),
//! marks it as downloaded from the internet and shows it in Finder or
//! Explorer; a download that saved nothing says why in an error dialog.
//! Downloads the webview still makes itself (a file the page made, `blob:`;
//! an attachment window on macOS) get the same feedback.
mod text;

pub use text::sentence;

use crate::web_proxy::download_to;
use std::{collections::HashSet, path::Path, sync::Mutex};
use tauri::{Manager, Webview};

/// The initialization script of a server's page that hands its download links
/// to `desktop_download`. See the file.
pub const SCRIPT: &str = include_str!("../page-shim/app-downloads.js");

/// The servers (by page) with a download under way.
static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// One download of one server at a time: a second click while the first runs
/// would only race it for the same file (and for a data export, which the
/// server hands to one download at a time).
struct Running(String);

impl Running {
    fn start(server: &str) -> Option<Running> {
        let mut set = RUNNING.lock().ok()?;
        set.get_or_insert_with(HashSet::new)
            .insert(server.to_owned())
            .then(|| Running(server.to_owned()))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(mut set) = RUNNING.lock()
            && let Some(set) = set.as_mut()
        {
            set.remove(&self.0);
        }
    }
}

/// Saves `path` (the instance's `/api/`, as the link of the calling page named
/// it) into the Downloads folder; `name` is the link's `download` attribute.
/// Gives the saved path. The server is the calling page's, never an argument.
#[tauri::command]
pub async fn desktop_download(
    webview: Webview,
    path: String,
    name: String,
) -> Result<String, String> {
    let session = crate::web_client::instance_page(&webview)?;
    let fail = |text: String| {
        crate::web_client::show_error(&text);
        Err(text)
    };
    let Some(_running) = Running::start(webview.label()) else {
        return fail(text::text("busy"));
    };
    let app = webview.app_handle();
    let Ok(dir) = app.path().download_dir() else {
        return fail(text::text("no_folder"));
    };
    let agent = app.package_info().name.clone();
    match download_to(&session.instance, &path, &dir, &name, &agent).await {
        Ok(saved) => {
            reveal(&saved);
            Ok(saved.display().to_string())
        }
        Err(failure) => fail(sentence(&failure)),
    }
}

/// Where the downloads the webview makes itself (macOS) go, by address, from
/// the moment the host decided until the download ended: WKWebView reports
/// the end without the path. `None`: the host refused it and already said why.
#[cfg(target_os = "macos")]
static UNDER_WAY: Mutex<Vec<(String, Option<std::path::PathBuf>)>> = Mutex::new(Vec::new());

/// A download the webview makes itself will be saved at `path`, or (`None`)
/// was refused with an error already shown.
#[cfg(target_os = "macos")]
pub fn started(url: &url::Url, path: Option<&Path>) {
    if let Ok(mut list) = UNDER_WAY.lock() {
        list.push((url.to_string(), path.map(Path::to_path_buf)));
    }
}

/// The error for a download the webview makes itself that has no folder to go
/// to (shown once: its end then says nothing more).
#[cfg(target_os = "macos")]
pub fn no_folder(url: &url::Url) {
    crate::web_client::show_error(&text::text("no_folder"));
    started(url, None);
}

/// The outcome of a download the webview made itself: the saved file is
/// marked and shown, a failed one is reported (unless the host refused it).
#[cfg(target_os = "macos")]
pub fn finished(app: &tauri::AppHandle, url: &url::Url, path: Option<&Path>, success: bool) {
    let picked = UNDER_WAY.lock().ok().and_then(|mut list| {
        let at = list.iter().position(|(u, _)| u == url.as_str())?;
        Some(list.remove(at).1)
    });
    if matches!(picked, Some(None)) {
        return;
    }
    match path.map(Path::to_path_buf).or(picked.flatten()) {
        Some(path) if success && path.exists() => {
            crate::web_proxy::mark(&path, &app.package_info().name);
            reveal(&path);
        }
        _ => crate::web_client::show_error(&text::text("webview_failed")),
    }
}

/// Shows a saved file in the file manager, selected: the confirmation that
/// it was saved and where.
fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Explorer reads `/select,"<path>"` as one argument, quotes included.
        let _ = std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_runs_one_download_at_a_time() {
        let first = Running::start("i-test-one").expect("free");
        assert!(Running::start("i-test-one").is_none(), "a second one waits");
        let other = Running::start("i-test-two");
        assert!(other.is_some(), "another server is not held up");
        drop(first);
        assert!(
            Running::start("i-test-one").is_some(),
            "free again after the first ended"
        );
    }
}
