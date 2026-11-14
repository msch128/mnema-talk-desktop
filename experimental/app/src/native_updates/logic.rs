//! The decisions of the self-update that need no app: the schedule, which versions
//! are offered, what "Update now" does, where an installer may come from and the
//! signature check.
use super::state::Phase;
use crate::web_bundle::{CHECK_INTERVAL_SECS, Version};
use base64::Engine as _;
use std::path::Path;
use url::Url;

/// The plugin buffers the whole installer in memory before its signature can
/// be checked, so a download beyond this is abandoned.
pub(super) const MAX_INSTALLER: u64 = 512 * 1024 * 1024;

/// Shipped only inside the portable ZIP, next to the executable, by the release
/// workflow. The installer never carries it. A copy with the marker never runs
/// the installer (a second copy would be installed beside it): it replaces its
/// own files from the signed portable ZIP instead.
pub const PORTABLE_MARKER: &str = "mnema-portable.marker";

/// The entry of the feed that names the signed portable ZIP.
pub const PORTABLE_TARGET: &str = "windows-x86_64-portable";

/// The page of a release, linked from the update window.
pub(super) const RELEASE_PAGE: &str =
    "https://github.com/msch128/mnema-talk-desktop/releases/tag/v";

pub fn disabled() -> bool {
    crate::web_updates::disabled()
}

/// Whether the executable at `exe` is the portable build: the marker is a
/// regular file in the same directory (a link or a directory does not count).
pub fn portable_at(exe: &Path) -> bool {
    exe.parent()
        .and_then(|dir| std::fs::symlink_metadata(dir.join(PORTABLE_MARKER)).ok())
        .is_some_and(|m| m.file_type().is_file())
}

/// Whether the running program is the portable build. An unknown location is
/// not portable: only the marker switches the installer off.
pub fn is_portable() -> bool {
    std::env::current_exe().is_ok_and(|exe| portable_at(&exe))
}

/// The release page of a version, only for a plain `MAJOR.MINOR.PATCH`.
pub fn release_page(version: &str) -> Option<Url> {
    let version = Version::parse(version)?;
    Url::parse(&format!("{RELEASE_PAGE}{version}")).ok()
}

// ---------------------------------------------------------------- pure logic

/// Whether the interval has passed. A clock set back counts as due.
pub fn check_due(last_check: u64, now: u64) -> bool {
    last_check > now || now - last_check >= CHECK_INTERVAL_SECS
}

/// What "Update now" does.
#[derive(Debug, PartialEq, Eq)]
pub enum NowChoice {
    /// Download, verify, install and restart.
    Install,
    /// A Talk is running: ask first ("after the call" or "update now").
    AskAboutTalk,
    /// Already verified and waiting for the Talk to end: stop waiting.
    StopWaiting,
    /// Nothing to install (checking, busy or up to date): show the window.
    Show,
}

/// "Update now" in `phase`. `confirmed` is the answer "update now anyway" to
/// the question about the running Talk.
pub fn now_choice(phase: Phase, offered: bool, in_talk: bool, confirmed: bool) -> NowChoice {
    match phase {
        Phase::Waiting => NowChoice::StopWaiting,
        Phase::Available | Phase::Ready | Phase::Failed if offered => {
            if in_talk && !confirmed {
                NowChoice::AskAboutTalk
            } else {
                NowChoice::Install
            }
        }
        _ => NowChoice::Show,
    }
}

/// How many polls in a row must see no Talk before a waiting update installs:
/// a page that misses one report is not taken for the end of the Talk.
pub const QUIET_POLLS: u32 = 3;

/// Whether the wait for the end of the Talk is over.
pub fn call_over(quiet_polls: u32, forced: bool) -> bool {
    forced || quiet_polls >= QUIET_POLLS
}

/// The candidate version when it is strictly newer than `current`. Native
/// versions are plain `MAJOR.MINOR.PATCH`; anything else is not offered.
pub fn offerable(current: &str, candidate: &str) -> Option<Version> {
    let candidate = Version::parse(candidate)?;
    (candidate > Version::parse(current)?).then_some(candidate)
}

/// A skipped version stays quiet; a newer one is offered again.
pub fn is_skipped(skipped: Option<&str>, candidate: &str) -> bool {
    skipped == Some(candidate)
}

/// The installer is fetched from GitHub's release downloads over HTTPS (which
/// redirects to GitHub's asset hosts). The signature decides trust; this keeps
/// the request from going elsewhere in the first place.
pub fn download_url_ok(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    Key,
    Signature,
    Mismatch,
    Version,
}

/// Minisign verification of a package, the same rule the plugin applies to a
/// fresh download, for the installer that waited on disk.
pub fn verify_package(
    bytes: &[u8],
    signature: &str,
    pubkey: &str,
    version: &str,
) -> Result<(), VerifyError> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = |s: &str| {
        b64.decode(s.trim())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let key = text(pubkey)
        .and_then(|t| minisign_verify::PublicKey::decode(&t).ok())
        .ok_or(VerifyError::Key)?;
    let sig = text(signature)
        .and_then(|t| minisign_verify::Signature::decode(&t).ok())
        .ok_or(VerifyError::Signature)?;
    key.verify(bytes, &sig, true)
        .map_err(|_| VerifyError::Mismatch)?;
    // Only after `verify` is the trusted comment covered by the signature.
    let signed = sig
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
        .ok_or(VerifyError::Version)?;
    let signed = Version::parse(signed.trim_start_matches('v')).ok_or(VerifyError::Version)?;
    (Version::parse(version) == Some(signed))
        .then_some(())
        .ok_or(VerifyError::Version)
}

/// Whether an installer download is already beyond what is accepted, by its
/// announced length or by what has arrived.
pub fn over_cap(received: u64, total: Option<u64>) -> bool {
    received > MAX_INSTALLER || total.is_some_and(|t| t > MAX_INSTALLER)
}
