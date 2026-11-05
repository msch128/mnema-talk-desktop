//! Links that leave the app, and invite links that may stay in it.
use super::{
    address::{address, path_invite},
    app_api, bundled,
    dialog::{Dialog, show_dialog},
};
use url::Url;

pub(super) fn external_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => {
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.as_str().len() <= 4096
        }
        // The web app links an e-mail address in a message as `mailto:`. No
        // other scheme (tel:, file:, an app's own) leaves the app.
        "mailto" => {
            !url.path().is_empty()
                && url.as_str().len() <= 2048
                && !url.as_str().chars().any(char::is_control)
        }
        _ => false,
    }
}

/// Lets a navigation of a server's page happen when it stays in the app;
/// a `mailto:` link goes to the mail program after a confirmation instead.
pub(crate) fn navigation_allowed(url: &Url) -> bool {
    if bundled(url) || app_api(url) {
        return true;
    }
    if url.scheme() == "mailto" {
        open_external(url);
    }
    false
}

/// Asks before a link leaves the app, then hands it to the system browser (a mailto: address to the mail program).
pub(crate) fn open_external(url: &Url) {
    if !external_allowed(url) {
        return;
    }
    #[cfg(target_os = "windows")]
    if show_dialog(Dialog::Confirm, &confirm_text(url)) {
        super::dialog::windows_dialog::open(url.as_str());
    }
    // The dialog waits for an answer: not on the thread that runs the window.
    #[cfg(target_os = "macos")]
    {
        let url = url.clone();
        std::thread::spawn(move || {
            if show_dialog(Dialog::Confirm, &confirm_text(&url)) {
                let _ = std::process::Command::new("/usr/bin/open")
                    .arg(url.as_str())
                    .status();
            }
        });
    }
}

/// What a link of a server's page that is not part of the app is for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Link {
    /// Open it in the browser, after the usual question.
    Browser,
    /// An invite to a server that is in the list: go there and show the join screen.
    Known { origin: Url, code: String },
    /// An invite to a server that is not in the list, which has room: offer to add it.
    Add { code: String },
}

/// Decides what to do with a link: an invite link (`https://host/invite/code`)
/// is for the app when its server is in `listed` or can be added (`room`);
/// anything else, or any invite when the list is full, goes to the browser.
/// The host is read as the address field would read it.
pub(crate) fn invite_decision(link: &Url, listed: &[Url], room: bool) -> Link {
    if !external_allowed(link) || link.scheme() != "https" {
        return Link::Browser;
    }
    let Some(code) = path_invite(link.path()) else {
        return Link::Browser;
    };
    let Ok(origin) = address(link.as_str()) else {
        return Link::Browser;
    };
    if listed.contains(&origin) {
        Link::Known { origin, code }
    } else if room {
        Link::Add { code }
    } else {
        Link::Browser
    }
}

/// Asks whether to add the server behind an invite link. Blocks: not on the
/// thread that runs the window.
pub(crate) fn confirm_add(link: &Url) -> bool {
    show_dialog(
        Dialog::Add,
        &format!(
            "Add this server?\n\n{}",
            link.host_str().unwrap_or_default()
        ),
    )
}

pub(super) fn confirm_text(url: &Url) -> String {
    // The whole address, query included: a mailto: can carry cc, bcc and a
    // body the person would not see otherwise.
    if url.scheme() == "mailto" {
        return format!(
            "Write to this address in your mail program?\n\n{}",
            url.as_str()
        );
    }
    format!("Open this link in your browser?\n\n{}", url.as_str())
}
