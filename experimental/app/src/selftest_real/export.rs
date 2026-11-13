//! The data export through the page's own download link: the host saves it
//! into the Downloads folder and the server records the download.
use super::{Target, page::*};
use std::{path::PathBuf, time::SystemTime};
use tauri::Manager;

/// Files of the export the server names `mnema-talk-export.zip`, saved after `since`.
fn saved_exports(dir: &std::path::Path, since: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("mnema-talk-export") && name.ends_with(".zip")
        })
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t >= since)
        })
        .map(|e| e.path())
        .collect()
}

impl Target<'_> {
    /// Asks for an export, waits until it is ready, clicks the link the web
    /// app shows for it and checks the ZIP in the Downloads folder.
    pub(super) async fn export(&self) {
        let asked = self
            .page(&format!(
                "return (await api('POST','/api/users/me/export',{{current_password:{:?}}})).s",
                self.account.password
            ))
            .await;
        check("an export is requested", asked == "202", &asked);
        let mut state = String::new();
        for _ in 0..120 {
            state = self
                .page("return (await api('GET','/api/users/me/export')).j.state")
                .await;
            if state != "\"pending\"" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        check("the export is built", state == "\"ready\"", &state);
        let Ok(dir) = self.app.path().download_dir() else {
            check("there is a Downloads folder", false, "none");
            return;
        };
        let since = SystemTime::now() - std::time::Duration::from_secs(2);
        // The link as the web app renders it, clicked like a person would.
        let clicked = self
            .page("const a=document.createElement('a');a.href='/api/users/me/export/download';a.setAttribute('download','');a.textContent='Download';document.body.append(a);a.click();a.remove();return 'clicked'")
            .await;
        check(
            "the download link is clicked",
            clicked == "\"clicked\"",
            &clicked,
        );
        let mut saved = Vec::new();
        for _ in 0..600 {
            saved = saved_exports(&dir, since);
            if !saved.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let zip = saved
            .first()
            .and_then(|p| std::fs::read(p).ok())
            .is_some_and(|bytes| bytes.starts_with(b"PK"));
        check(
            "the export is saved as a ZIP in the Downloads folder",
            zip,
            format!("{saved:?} in {}", dir.display()),
        );
        // A server from before the hour to download again reads "downloaded".
        // The server writes it down right after the last byte left it.
        let mut after = String::new();
        for _ in 0..50 {
            after = self
                .page("const j=(await api('GET','/api/users/me/export')).j;return j.state==='downloaded'||(j.state==='ready'&&!!j.downloaded_at)")
                .await;
            if after == "true" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        check("the server records the download", after == "true", &after);
        for path in saved {
            let _ = std::fs::remove_file(path);
        }
    }
}
