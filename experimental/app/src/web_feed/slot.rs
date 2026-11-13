//! What one read of a server does to its place on the rail: the two numbers,
//! and a quiet toast when its mentions rise.
use super::Read;
use crate::instances::{Phase, Registry};
use crate::web_client::Trusted;
use tauri::Manager;

/// How many new mentions a quiet toast announces: one toast for each rise of
/// the count, none for a muted server.
pub fn mention_rise(before: u32, after: u32, muted: bool) -> Option<u32> {
    (!muted && after > before).then(|| after - before)
}

/// What the quiet toast says.
pub fn mention_text(count: u32, server: &str) -> String {
    let noun = if count == 1 { "mention" } else { "mentions" };
    format!("{count} new {noun} on {server}")
}

/// Puts the two numbers on the rail, and says a rise of the mentions in a
/// quiet toast (the server's mute and the system's setting apply). A
/// signed-out session clears them. Gives whether the server is still there
/// to report for.
pub(super) fn report(app: &tauri::AppHandle, id: &str, read: Read) -> bool {
    let trusted = app.state::<Trusted>();
    let applied = match trusted.registry.lock() {
        Ok(mut registry) => apply(&mut registry, id, read, crate::web_proxy::now()),
        Err(_) => return false,
    };
    let Applied::Shown(rise) = applied else {
        return applied == Applied::Kept;
    };
    crate::web_shell::counts_changed(app);
    if let Some((count, name)) = rise {
        crate::web_notify::quiet_toast(app, &name, &mention_text(count, &name));
    }
    true
}

/// What one read did to the server's slot.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The server is gone: the feed ends.
    Gone,
    /// Nothing changed on the rail (no answer, or the server is not running).
    Kept,
    /// The counts are on the rail, with a rise of the mentions to announce
    /// (how many, and the server's name).
    Shown(Option<(u32, String)>),
}

/// Puts what was read on the slot of `id`; a signed-out session clears its
/// counts.
pub(crate) fn apply(registry: &mut Registry, id: &str, read: Read, now: i64) -> Applied {
    let (unread, mentions) = match read {
        Read::Counts(unread, mentions) => (unread, mentions),
        Read::SignedOut => (0, 0),
        Read::Unreachable => return Applied::Kept,
    };
    let Some(slot) = registry.slot(id) else {
        return Applied::Gone;
    };
    if !matches!(slot.phase, Phase::Ready(_)) {
        return Applied::Kept;
    }
    // The first numbers since the server started (or since a restart) are
    // what is there already, not news: they announce nothing.
    let (before, name, first) = (slot.mentions, slot.name(), !slot.reported);
    let muted = registry.is_muted(id, now) || first;
    registry.report(id, unread, mentions);
    Applied::Shown(
        mention_rise(before, mentions.min(crate::instances::MAX_COUNT), muted).map(|n| (n, name)),
    )
}
