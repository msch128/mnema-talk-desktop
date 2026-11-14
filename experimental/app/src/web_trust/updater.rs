//! Which Tauri updater key an update may be signed with, given the statement
//! in force.
use super::statement::Statement;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::sync::Mutex;

fn decode_text(wrapped: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(wrapped.trim()).ok()?).ok()
}

/// Key id (8 bytes, hex, file order) of a minisign public key in the form of
/// `tauri.conf.json`: base64 of the `.pub` file.
pub fn minisign_key_id(pubkey: &str) -> Option<String> {
    let line = decode_text(pubkey)?.lines().nth(1)?.to_owned();
    let raw = STANDARD.decode(line.trim()).ok()?;
    (raw.len() == 42 && raw[0] == b'E' && raw[1] == b'd').then(|| hex::encode(&raw[2..10]))
}

/// Key id named by a minisign signature (base64 of the `.sig` file).
pub fn signature_key_id(signature: &str) -> Option<String> {
    let line = decode_text(signature)?.lines().nth(1)?.to_owned();
    let raw = STANDARD.decode(line.trim()).ok()?;
    (raw.len() == 74 && raw[0] == b'E' && (raw[1] == b'd' || raw[1] == b'D'))
        .then(|| hex::encode(&raw[2..10]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdaterRefusal {
    /// The signature names a key the statement revokes.
    Revoked,
    /// The statement lists updater keys and the signature names none of them.
    NotAllowed,
    Malformed,
}

/// The public key an update signed with `signature` must verify against.
/// Without a statement (or one that lists no updater key) that is the pinned
/// key. With one, the signature must name a listed, not revoked key, which may
/// replace the pinned one.
pub fn choose_updater_key(
    pinned: &str,
    statement: Option<&Statement>,
    signature: &str,
) -> Result<String, UpdaterRefusal> {
    let Some(statement) = statement else {
        return Ok(pinned.to_owned());
    };
    let id = signature_key_id(signature).ok_or(UpdaterRefusal::Malformed)?;
    if statement.revoked_updater_keys.contains(&id) {
        return Err(UpdaterRefusal::Revoked);
    }
    if statement.updater_keys.is_empty() {
        return Ok(pinned.to_owned());
    }
    statement
        .updater_keys
        .iter()
        .find(|k| minisign_key_id(k).as_deref() == Some(id.as_str()))
        .cloned()
        .ok_or(UpdaterRefusal::NotAllowed)
}

static CURRENT: Mutex<Option<Statement>> = Mutex::new(None);

/// Makes the statement in force visible to the native updater.
pub fn publish(statement: Option<&Statement>) {
    // The update self-test signs its feed with a throwaway key that no real
    // statement lists; only that test build (never shipped) leaves it out.
    if cfg!(feature = "update-selftest") {
        return;
    }
    if let Ok(mut current) = CURRENT.lock() {
        *current = statement.cloned();
    }
}

/// [`choose_updater_key`] against the statement in force.
pub fn updater_key(pinned: &str, signature: &str) -> Result<String, UpdaterRefusal> {
    let current = CURRENT.lock().map_err(|_| UpdaterRefusal::Malformed)?;
    choose_updater_key(pinned, current.as_ref(), signature)
}
