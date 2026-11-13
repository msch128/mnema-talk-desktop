//! Downloads of a server's page. A download link of the app's own API is
//! handed to the host by a page shim (`page-shim/app-downloads.js`): the host
//! streams the file into the Downloads folder (`web_proxy::download_to`), then
//! shows it in Finder or Explorer; a download that saved nothing says why in
//! an error dialog. Downloads the webview still makes itself (a file the page
//! made, `blob:`; an attachment window on macOS) get the same feedback.
use crate::web_proxy::{Failure, download_to};
use std::path::Path;
use tauri::{Manager, Webview};

/// The initialization script of a server's page that hands its download links
/// to `desktop_download`. See the file.
pub const SCRIPT: &str = include_str!("../page-shim/app-downloads.js");

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
    let app = webview.app_handle().clone();
    let Ok(dir) = app.path().download_dir() else {
        let text = "There is no Downloads folder to save the file in.";
        crate::web_client::show_error(text);
        return Err(text.into());
    };
    match download_to(&session.instance, &path, &dir, &name).await {
        Ok(saved) => {
            reveal(&saved);
            Ok(saved.display().to_string())
        }
        Err(failure) => {
            let text = sentence(&failure);
            crate::web_client::show_error(&text);
            Err(text)
        }
    }
}

/// What the error dialog says about a download that saved nothing.
pub fn sentence(failure: &Failure) -> String {
    match failure {
        Failure::Path => "This link cannot be downloaded.".into(),
        Failure::Unreachable => "The server could not be reached; nothing was saved.".into(),
        Failure::Refused(404, _) => {
            "There is nothing to download: the file was downloaded already, removed or has expired."
                .into()
        }
        Failure::Refused(_, Some(message)) => {
            format!("The server refused the download: {message}.")
        }
        Failure::Refused(status, None) => format!("The download failed (HTTP {status})."),
        Failure::TooLarge => "The file is too large for this client; nothing was saved.".into(),
        Failure::Cut => "The download was interrupted; nothing was saved.".into(),
        Failure::Disk(reason) => format!("The file could not be saved: {reason}."),
    }
}

/// Where the downloads the webview makes itself (macOS) go, by address, from
/// the moment the host picked the file until the download ended: WKWebView
/// reports the end without the path.
#[cfg(target_os = "macos")]
static UNDER_WAY: std::sync::Mutex<Vec<(String, std::path::PathBuf)>> =
    std::sync::Mutex::new(Vec::new());

/// A download the webview makes itself will be saved at `path`.
#[cfg(target_os = "macos")]
pub fn started(url: &url::Url, path: &Path) {
    if let Ok(mut list) = UNDER_WAY.lock() {
        list.push((url.to_string(), path.to_path_buf()));
    }
}

/// The outcome of a download the webview made itself: the saved file is
/// shown, a failed one is reported.
#[cfg(target_os = "macos")]
pub fn finished(url: &url::Url, path: Option<&Path>, success: bool) {
    let picked = UNDER_WAY.lock().ok().and_then(|mut list| {
        let at = list.iter().position(|(u, _)| u == url.as_str())?;
        Some(list.remove(at).1)
    });
    match path.map(Path::to_path_buf).or(picked) {
        Some(path) if success && path.exists() => reveal(&path),
        _ => crate::web_client::show_error("The download failed; nothing was saved."),
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
    fn every_failure_says_that_nothing_was_saved_or_why() {
        let spent = sentence(&Failure::Refused(
            404,
            Some("there is no export to download".into()),
        ));
        assert!(spent.contains("downloaded already"), "{spent}");
        assert_eq!(
            sentence(&Failure::Refused(
                403,
                Some("the download cannot be prefetched".into())
            )),
            "The server refused the download: the download cannot be prefetched."
        );
        assert_eq!(
            sentence(&Failure::Refused(500, None)),
            "The download failed (HTTP 500)."
        );
        for failure in [Failure::Unreachable, Failure::TooLarge, Failure::Cut] {
            assert!(
                sentence(&failure).contains("nothing was saved"),
                "{failure:?}"
            );
        }
        assert!(sentence(&Failure::Disk("disk full".into())).contains("disk full"));
    }
}
