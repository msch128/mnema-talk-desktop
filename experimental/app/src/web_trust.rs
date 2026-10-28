//! Signed trust statement: remote revocation of the CI signing keys
//! (mnema-talk `doc/desktop-trusted-client.md`, "Key revocation").
//!
//! The client pins one offline root key. A statement signed by it names the
//! web bundle keys and Tauri updater keys that are allowed and the ones that
//! are revoked. Where the bytes come from decides nothing: the signature is
//! checked before anything is parsed, and a statement with a lower serial than
//! the highest one accepted is refused.
use crate::web_bundle::{TrustedKey, Version, is_hex_id16, parse_key, write_atomic};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Raw Ed25519 root public key (`desktop-trust/root.pub` in mnema-talk).
const ROOT: &str = "0c6b31c234adbeb1449c63acd03fcd044d7b15b640607f9943a1e3f6a819ed77";

pub const TRUST_URL: &str =
    "https://raw.githubusercontent.com/msch128/mnema-talk/main/desktop-trust/trust.json";
pub const TRUST_SIG_URL: &str =
    "https://raw.githubusercontent.com/msch128/mnema-talk/main/desktop-trust/trust.json.sig";
pub const MAX_STATEMENT: usize = 64 << 10;

const STATEMENT_FILE: &str = "trust.json";
const SIGNATURE_FILE: &str = "trust.json.sig";

pub fn root_key() -> VerifyingKey {
    let raw: [u8; 32] = hex::decode(ROOT)
        .ok()
        .and_then(|b| b.try_into().ok())
        .expect("the pinned root key is well formed");
    VerifyingKey::from_bytes(&raw).expect("the pinned root key is a valid point")
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statement {
    pub schema: u32,
    pub serial: u64,
    pub issued_at: String,
    pub expires_at: String,
    pub bundle_keys: Vec<String>,
    pub revoked_bundle_keys: Vec<String>,
    pub updater_keys: Vec<String>,
    pub revoked_updater_keys: Vec<String>,
    /// Optional caps on the rollback marks, per web client API number.
    #[serde(default)]
    pub max_version: BTreeMap<String, String>,
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ` (a real calendar date),
/// nothing else.
pub fn parse_utc(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() != 20
        || [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .any(|&(i, c)| b[i] != c)
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let digits = text.get(from..to)?;
        digits
            .bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) || hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's civil-to-days).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// Longest validity a statement may claim (the signing tool's bound).
const MAX_VALIDITY_SECS: u64 = 400 * 86400;

/// Verifies the signature over the exact bytes, then parses and validates.
pub fn verify_statement(doc: &[u8], sig: &[u8], root: &VerifyingKey) -> Result<Statement, String> {
    if doc.len() > MAX_STATEMENT {
        return Err("trust statement too large".into());
    }
    let sig: [u8; 64] = STANDARD
        .decode(String::from_utf8_lossy(sig).trim())
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("malformed trust signature")?;
    root.verify_strict(doc, &Signature::from_bytes(&sig))
        .map_err(|_| "trust signature does not match the root key")?;
    let statement: Statement =
        serde_json::from_slice(doc).map_err(|_| "unreadable trust statement")?;
    statement.validate()?;
    Ok(statement)
}

impl Statement {
    fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.serial < 1 {
            return Err("unsupported trust statement".into());
        }
        let issued = parse_utc(&self.issued_at).ok_or("invalid issued_at")?;
        let expires = parse_utc(&self.expires_at).ok_or("invalid expires_at")?;
        if expires <= issued || expires - issued > MAX_VALIDITY_SECS {
            return Err("invalid validity period".into());
        }
        for (api, version) in &self.max_version {
            if api.parse::<u32>().ok().filter(|n| *n >= 1).is_none()
                || Version::parse(version).is_none()
            {
                return Err("invalid max_version".into());
            }
        }
        if self.bundle_keys.is_empty() {
            return Err("trust statement lists no bundle key".into());
        }
        if !self.revoked_bundle_keys.iter().all(|id| is_hex_id16(id))
            || !self.revoked_updater_keys.iter().all(|id| is_hex_id16(id))
        {
            return Err("invalid revoked key id".into());
        }
        for raw in &self.bundle_keys {
            let key = parse_key(raw).ok_or("invalid bundle key")?;
            if self.revoked_bundle_keys.contains(&key.id) {
                return Err("a bundle key is both allowed and revoked".into());
            }
        }
        for key in &self.updater_keys {
            let id = minisign_key_id(key).ok_or("invalid updater key")?;
            if self.revoked_updater_keys.contains(&id) {
                return Err("an updater key is both allowed and revoked".into());
            }
        }
        Ok(())
    }

    /// The caps as numbers (validated when the statement was verified).
    pub fn caps(&self) -> BTreeMap<u32, Version> {
        self.max_version
            .iter()
            .filter_map(|(api, v)| Some((api.parse().ok()?, Version::parse(v)?)))
            .collect()
    }

    /// Whether the statement is past `expires_at` at `now` (unix seconds).
    pub fn expired(&self, now: u64) -> bool {
        parse_utc(&self.expires_at).is_none_or(|e| now >= e)
    }

    /// Allowed web bundle keys that are not revoked.
    pub fn bundle_keys(&self) -> Vec<TrustedKey> {
        self.bundle_keys
            .iter()
            .filter_map(|raw| parse_key(raw))
            .filter(|k| !self.revoked_bundle_keys.contains(&k.id))
            .collect()
    }
}

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
    if let Ok(mut current) = CURRENT.lock() {
        *current = statement.cloned();
    }
}

/// [`choose_updater_key`] against the statement in force.
pub fn updater_key(pinned: &str, signature: &str) -> Result<String, UpdaterRefusal> {
    let current = CURRENT.lock().map_err(|_| UpdaterRefusal::Malformed)?;
    choose_updater_key(pinned, current.as_ref(), signature)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    Updated,
    Unchanged,
}

/// What a release knows at build time, so a client with no stored statement
/// (a fresh install, a removed file) cannot be talked back into an older state.
#[derive(Debug, Clone, Copy)]
pub struct Floor {
    /// Statements with a lower serial are refused.
    pub min_serial: u64,
    /// Key ids no statement may allow again.
    pub revoked_bundle_keys: &'static [&'static str],
    pub revoked_updater_keys: &'static [&'static str],
}

impl Floor {
    #[cfg(test)]
    pub const NONE: Floor = Floor {
        min_serial: 0,
        revoked_bundle_keys: &[],
        revoked_updater_keys: &[],
    };
}

/// The floor of this release: the serial of the statement it shipped with
/// (`fixtures/trust.json`). Raise it, and list the key ids revoked since, when
/// a later statement revokes a key.
pub const RELEASE_FLOOR: Floor = Floor {
    min_serial: 3,
    revoked_bundle_keys: &[],
    revoked_updater_keys: &[],
};

/// Statement, signature and the revocations carried forward, in one file so a
/// crash can never leave a new signature next to an old document.
const STATE_FILE: &str = "trust-state.json";

#[derive(serde::Serialize, Deserialize)]
struct Stored {
    statement: String,
    signature: String,
    #[serde(default)]
    revoked_bundle_keys: Vec<String>,
    #[serde(default)]
    revoked_updater_keys: Vec<String>,
}

struct Held {
    statement: Statement,
    doc: Vec<u8>,
    sig: Vec<u8>,
}

/// The last good statement, persisted next to the bundle cache.
pub struct TrustState {
    dir: Option<PathBuf>,
    root: VerifyingKey,
    floor: Floor,
    current: Option<Held>,
    /// Every key id any accepted statement (or the floor) revoked. A later
    /// statement can neither allow one again nor drop it from the lists in force.
    revoked_bundle: BTreeSet<String>,
    revoked_updater: BTreeSet<String>,
    /// The state in force is on disk.
    stored: bool,
}

impl TrustState {
    /// Loads and re-verifies the stored statement; a damaged or foreign file is
    /// ignored, which falls back to the compiled-in keys.
    #[cfg(test)]
    pub fn load(dir: Option<&Path>, root: VerifyingKey) -> TrustState {
        Self::load_with(dir, root, Floor::NONE)
    }

    pub fn load_with(dir: Option<&Path>, root: VerifyingKey, floor: Floor) -> TrustState {
        let mut state = TrustState {
            dir: dir.map(Path::to_path_buf),
            root,
            floor,
            current: None,
            revoked_bundle: floor
                .revoked_bundle_keys
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            revoked_updater: floor
                .revoked_updater_keys
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            stored: true,
        };
        if let Some((doc, sig, extra_bundle, extra_updater)) =
            dir.and_then(|d| state.read_stored(d))
            && let Ok(statement) = verify_statement(&doc, &sig, &state.root)
            // A statement below the floor is an older release's.
            && statement.serial >= floor.min_serial
        {
            state.revoked_bundle.extend(extra_bundle);
            state.revoked_updater.extend(extra_updater);
            state.hold(statement, doc, sig);
        }
        state
    }

    /// The stored bytes: the state file, else the pair an older client wrote.
    #[allow(clippy::type_complexity)]
    fn read_stored(&self, dir: &Path) -> Option<(Vec<u8>, Vec<u8>, Vec<String>, Vec<String>)> {
        if let Ok(raw) = std::fs::read(dir.join(STATE_FILE)) {
            let stored: Stored = serde_json::from_slice(&raw).ok()?;
            let ids = |list: Vec<String>| list.into_iter().filter(|id| is_hex_id16(id)).collect();
            return Some((
                STANDARD.decode(stored.statement).ok()?,
                stored.signature.into_bytes(),
                ids(stored.revoked_bundle_keys),
                ids(stored.revoked_updater_keys),
            ));
        }
        let doc = std::fs::read(dir.join(STATEMENT_FILE)).ok()?;
        let sig = std::fs::read(dir.join(SIGNATURE_FILE)).ok()?;
        Some((doc, sig, vec![], vec![]))
    }

    /// Puts a verified statement in force, with every revocation seen so far.
    fn hold(&mut self, mut statement: Statement, doc: Vec<u8>, sig: Vec<u8>) {
        self.revoked_bundle
            .extend(statement.revoked_bundle_keys.iter().cloned());
        self.revoked_updater
            .extend(statement.revoked_updater_keys.iter().cloned());
        statement.revoked_bundle_keys = self.revoked_bundle.iter().cloned().collect();
        statement.revoked_updater_keys = self.revoked_updater.iter().cloned().collect();
        self.current = Some(Held {
            statement,
            doc,
            sig,
        });
    }

    pub fn statement(&self) -> Option<&Statement> {
        self.current.as_ref().map(|h| &h.statement)
    }

    /// A statement is in force and has not expired. An expired one still
    /// decides which keys count (its revocations stay), but nothing new is
    /// installed while it is not fresh.
    pub fn fresh(&self, now: u64) -> bool {
        self.statement().is_some_and(|s| !s.expired(now))
    }

    /// Bundle keys in force: the statement's, else the compiled-in ones.
    pub fn effective_keys(&self, pinned: Vec<TrustedKey>) -> Vec<TrustedKey> {
        match self.statement() {
            Some(s) => s.bundle_keys(),
            None => pinned,
        }
    }

    /// Writes the state in force in one atomic step.
    fn persist(&self) -> std::io::Result<()> {
        let (Some(dir), Some(held)) = (&self.dir, &self.current) else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let stored = Stored {
            statement: STANDARD.encode(&held.doc),
            signature: String::from_utf8_lossy(&held.sig).into_owned(),
            revoked_bundle_keys: self.revoked_bundle.iter().cloned().collect(),
            revoked_updater_keys: self.revoked_updater.iter().cloned().collect(),
        };
        write_atomic(&dir.join(STATE_FILE), &serde_json::to_vec(&stored)?)?;
        // The pair an older client wrote must not outlive the state: a stale
        // one would be read back should the state file ever be lost.
        for name in [STATEMENT_FILE, SIGNATURE_FILE] {
            let _ = std::fs::remove_file(dir.join(name));
        }
        Ok(())
    }

    fn store(&mut self) {
        self.stored = self.persist().is_ok();
        if !self.stored {
            eprintln!("Mnema Desktop: cannot store the trust statement; it applies until restart");
        }
    }

    /// Takes a fetched statement. An expired one is never accepted, lower
    /// serials are refused, and the same serial only with identical bytes. A
    /// key an earlier statement revoked cannot be allowed again. The statement
    /// applies as soon as it verified; failing to store it is logged and
    /// retried with the next identical fetch.
    pub fn accept(&mut self, doc: &[u8], sig: &[u8], now: u64) -> Result<Accepted, String> {
        let statement = verify_statement(doc, sig, &self.root)?;
        if statement.expired(now) {
            return Err("trust statement has expired".into());
        }
        if statement.serial < self.floor.min_serial {
            return Err(format!(
                "trust statement {} is older than this release accepts",
                statement.serial
            ));
        }
        if let Some(held) = &self.current {
            if statement.serial < held.statement.serial {
                return Err(format!(
                    "trust statement {} is older than the {} already accepted",
                    statement.serial, held.statement.serial
                ));
            }
            if statement.serial == held.statement.serial {
                return if held.doc == doc {
                    if !self.stored {
                        self.store();
                    }
                    Ok(Accepted::Unchanged)
                } else {
                    Err("a different trust statement with the same serial".into())
                };
            }
        }
        if statement
            .bundle_keys
            .iter()
            .filter_map(|raw| parse_key(raw))
            .any(|k| self.revoked_bundle.contains(&k.id))
        {
            return Err("a revoked bundle key is allowed again".into());
        }
        if statement
            .updater_keys
            .iter()
            .filter_map(|k| minisign_key_id(k))
            .any(|id| self.revoked_updater.contains(&id))
        {
            return Err("a revoked updater key is allowed again".into());
        }
        self.hold(statement, doc.to_vec(), sig.to_vec());
        self.store();
        Ok(Accepted::Updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_bundle::fixture::{signing_key, trusted};
    use ed25519_dalek::Signer;

    const UPDATER: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDg3MUUyOTlDQUVCRjZCMkYKUldRdmE3K3VuQ2tlaDdOajlZaG9vbENvRFBXQmtQd1pKUjZmK1hwRW8rSCt4b04rZzhwWVY3SkEK";

    /// 2026-11-01T00:00:00Z
    const NOW: u64 = 1_793_491_200;

    fn root(seed: u8) -> ed25519_dalek::SigningKey {
        signing_key(seed)
    }

    fn key_hex(seed: u8) -> String {
        hex::encode(signing_key(seed).verifying_key().to_bytes())
    }

    fn key_id_of(seed: u8) -> String {
        crate::web_bundle::key_id(&signing_key(seed).verifying_key().to_bytes())
    }

    /// A statement allowing the bundle keys of the given seeds and revoking the
    /// keys of the revoked seeds.
    fn doc(serial: u64, allowed: &[u8], revoked: &[u8]) -> String {
        serde_json::json!({
            "schema": 1,
            "serial": serial,
            "issued_at": "2026-10-17T10:00:00Z",
            "expires_at": "2027-01-15T10:00:00Z",
            "bundle_keys": allowed.iter().map(|s| key_hex(*s)).collect::<Vec<_>>(),
            "revoked_bundle_keys": revoked.iter().map(|s| key_id_of(*s)).collect::<Vec<_>>(),
            "updater_keys": [],
            "revoked_updater_keys": [],
        })
        .to_string()
    }

    fn signed(key: &ed25519_dalek::SigningKey, doc: &str) -> (Vec<u8>, Vec<u8>) {
        let sig = STANDARD.encode(key.sign(doc.as_bytes()).to_bytes());
        (doc.as_bytes().to_vec(), sig.into_bytes())
    }

    fn state(seed: u8) -> TrustState {
        TrustState::load(None, root(seed).verifying_key())
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mnema-trust-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_pinned_root_key_is_well_formed() {
        let _ = root_key();
    }

    #[test]
    fn the_statement_from_the_signing_tool_verifies_with_the_pinned_root_key() {
        let doc = include_bytes!("../fixtures/trust.json");
        let sig = include_bytes!("../fixtures/trust.json.sig");
        let statement = verify_statement(doc, sig, &root_key()).expect("pinned root verifies");
        assert_eq!(statement.serial, 3);
        let keys = statement.bundle_keys();
        assert!(
            crate::web_bundle::trusted_keys()
                .iter()
                .all(|p| keys.iter().any(|k| k.id == p.id)),
            "every compiled-in key is allowed by the first statement"
        );
        let pinned_updater = minisign_key_id(UPDATER).unwrap();
        assert!(
            statement
                .updater_keys
                .iter()
                .any(|k| minisign_key_id(k).as_deref() == Some(pinned_updater.as_str()))
        );
    }

    #[test]
    fn a_statement_signed_by_the_root_key_is_accepted() {
        let (d, s) = signed(&root(9), &doc(1, &[1], &[]));
        let mut trust = state(9);
        assert_eq!(trust.accept(&d, &s, NOW), Ok(Accepted::Updated));
        assert_eq!(trust.statement().unwrap().serial, 1);
    }

    #[test]
    fn a_statement_signed_by_another_key_is_refused() {
        let (d, s) = signed(&root(8), &doc(1, &[1], &[]));
        let mut trust = state(9);
        assert!(trust.accept(&d, &s, NOW).is_err());
        assert!(trust.statement().is_none());
    }

    #[test]
    fn a_tampered_statement_or_signature_is_refused() {
        let (d, s) = signed(&root(9), &doc(1, &[1], &[]));
        let mut trust = state(9);
        let mut bad = d.clone();
        bad[10] ^= 1;
        assert!(trust.accept(&bad, &s, NOW).is_err());
        assert!(trust.accept(&d, b"AAAA", NOW).is_err());
        assert!(trust.accept(&d, b"", NOW).is_err());
        let mut bad_sig = s.clone();
        bad_sig[0] = if bad_sig[0] == b'A' { b'B' } else { b'A' };
        assert!(trust.accept(&d, &bad_sig, NOW).is_err());
        assert!(trust.statement().is_none());
    }

    #[test]
    fn a_correctly_signed_statement_must_still_be_well_formed() {
        let mut trust = state(9);
        for text in [
            doc(0, &[1], &[]),
            doc(1, &[], &[]),
            doc(1, &[1], &[1]),
            "{}".to_owned(),
            doc(1, &[1], &[]).replace("\"schema\":1", "\"schema\":2"),
            doc(1, &[1], &[]).replace("\"serial\"", "\"extra\":1,\"serial\""),
            doc(1, &[1], &[]).replace("2027-01-15", "2026-10-17"),
            doc(1, &[1], &[]).replace("2027-01-15", "2029-01-15"),
            doc(1, &[1], &[]).replace("2027-01-15T10:00:00Z", "soon"),
        ] {
            let (d, s) = signed(&root(9), &text);
            assert!(trust.accept(&d, &s, NOW).is_err(), "{text}");
        }
    }

    #[test]
    fn an_older_serial_is_refused_and_an_equal_one_changes_nothing() {
        let mut trust = state(9);
        let (d2, s2) = signed(&root(9), &doc(2, &[2], &[1]));
        let (d1, s1) = signed(&root(9), &doc(1, &[1], &[]));
        assert_eq!(trust.accept(&d2, &s2, NOW), Ok(Accepted::Updated));
        assert!(trust.accept(&d1, &s1, NOW).unwrap_err().contains("older"));
        assert_eq!(trust.accept(&d2, &s2, NOW), Ok(Accepted::Unchanged));
        let (dx, sx) = signed(&root(9), &doc(2, &[3], &[]));
        assert!(trust.accept(&dx, &sx, NOW).is_err());
        assert_eq!(trust.statement().unwrap().serial, 2);
    }

    #[test]
    fn a_revoked_key_is_dropped_and_a_new_one_is_allowed() {
        let mut trust = state(9);
        let pinned = trusted(1);
        assert_eq!(
            trust.effective_keys(trusted(1))[0].id,
            pinned[0].id,
            "default until a statement arrives"
        );
        let (d, s) = signed(&root(9), &doc(2, &[2], &[1]));
        trust.accept(&d, &s, NOW).unwrap();
        let keys = trust.effective_keys(trusted(1));
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, trusted(2)[0].id);
        assert!(keys.iter().all(|k| k.id != pinned[0].id));
    }

    #[test]
    fn the_last_good_statement_survives_a_restart_and_a_damaged_file_is_ignored() {
        let d = dir("persist");
        let (doc_bytes, sig) = signed(&root(9), &doc(3, &[2], &[1]));
        let mut trust = TrustState::load(Some(&d), root(9).verifying_key());
        assert!(trust.statement().is_none(), "offline, no statement yet");
        trust.accept(&doc_bytes, &sig, NOW).unwrap();
        let again = TrustState::load(Some(&d), root(9).verifying_key());
        assert_eq!(again.statement().unwrap().serial, 3);
        // The same files under another root key are not a statement.
        assert!(
            TrustState::load(Some(&d), root(8).verifying_key())
                .statement()
                .is_none()
        );
        let mut edited = doc_bytes.clone();
        edited[5] ^= 1;
        let mut stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(d.join(STATE_FILE)).unwrap()).unwrap();
        stored["statement"] = STANDARD.encode(edited).into();
        std::fs::write(d.join(STATE_FILE), stored.to_string()).unwrap();
        assert!(
            TrustState::load(Some(&d), root(9).verifying_key())
                .statement()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_statement_and_its_signature_are_stored_as_one_file() {
        let d = dir("one-file");
        let (doc_bytes, sig) = signed(&root(9), &doc(3, &[2], &[1]));
        let mut trust = TrustState::load(Some(&d), root(9).verifying_key());
        trust.accept(&doc_bytes, &sig, NOW).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [STATE_FILE],
            "no second file that could be torn from it"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_pair_an_older_client_stored_is_read_and_replaced() {
        let d = dir("legacy");
        let (doc_bytes, sig) = signed(&root(9), &doc(2, &[2], &[1]));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(STATEMENT_FILE), &doc_bytes).unwrap();
        std::fs::write(d.join(SIGNATURE_FILE), &sig).unwrap();
        let mut trust = TrustState::load(Some(&d), root(9).verifying_key());
        assert_eq!(trust.statement().unwrap().serial, 2);
        let (next_doc, next_sig) = signed(&root(9), &doc(3, &[2], &[1]));
        trust.accept(&next_doc, &next_sig, NOW).unwrap();
        assert!(!d.join(STATEMENT_FILE).exists() && !d.join(SIGNATURE_FILE).exists());
        let again = TrustState::load(Some(&d), root(9).verifying_key());
        assert_eq!(again.statement().unwrap().serial, 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_verified_statement_applies_even_when_it_cannot_be_stored() {
        let d = dir("unwritable");
        std::fs::create_dir_all(d.parent().unwrap()).unwrap();
        // A file where the directory should be: nothing can be stored.
        std::fs::write(&d, b"in the way").unwrap();
        let (doc_bytes, sig) = signed(&root(9), &doc(2, &[2], &[1]));
        let mut trust = TrustState::load(Some(&d), root(9).verifying_key());
        assert_eq!(trust.accept(&doc_bytes, &sig, NOW), Ok(Accepted::Updated));
        assert_eq!(trust.statement().unwrap().serial, 2);
        assert!(
            trust
                .effective_keys(trusted(1))
                .iter()
                .all(|k| k.id != trusted(1)[0].id),
            "the revocation is enforced in this process"
        );
        // Once the directory works, the same fetch stores it.
        std::fs::remove_file(&d).unwrap();
        assert_eq!(trust.accept(&doc_bytes, &sig, NOW), Ok(Accepted::Unchanged));
        let again = TrustState::load(Some(&d), root(9).verifying_key());
        assert_eq!(again.statement().unwrap().serial, 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_later_statement_cannot_allow_a_revoked_key_again() {
        let mut trust = state(9);
        let (d2, s2) = signed(&root(9), &doc(2, &[2], &[1]));
        trust.accept(&d2, &s2, NOW).unwrap();
        // Serial 3 was signed without carrying the revocation forward.
        let (d3, s3) = signed(&root(9), &doc(3, &[1, 2], &[]));
        assert!(
            trust
                .accept(&d3, &s3, NOW)
                .unwrap_err()
                .contains("allowed again")
        );
        assert_eq!(trust.statement().unwrap().serial, 2);
        // One that merely forgets to list it keeps the key revoked.
        let (d4, s4) = signed(&root(9), &doc(4, &[2, 3], &[]));
        trust.accept(&d4, &s4, NOW).unwrap();
        assert!(
            trust
                .effective_keys(trusted(1))
                .iter()
                .all(|k| k.id != key_id_of(1))
        );
        assert!(
            trust
                .statement()
                .unwrap()
                .revoked_bundle_keys
                .contains(&key_id_of(1))
        );
    }

    #[test]
    fn revocations_survive_a_restart_and_apply_to_updater_keys() {
        let d = dir("cumulative");
        let (old, old_pair) = pair();
        let (new, _) = pair();
        let old_id = minisign_key_id(&old).unwrap();
        let with_updaters = |serial: u64, allowed: &[&str], revoked: &[&str]| {
            let mut v: serde_json::Value = serde_json::from_str(&doc(serial, &[2], &[])).unwrap();
            v["updater_keys"] = allowed.to_vec().into();
            v["revoked_updater_keys"] = revoked.to_vec().into();
            v.to_string()
        };
        let mut trust = TrustState::load(Some(&d), root(9).verifying_key());
        let (d1, s1) = signed(&root(9), &with_updaters(1, &[&new], &[&old_id]));
        trust.accept(&d1, &s1, NOW).unwrap();
        let (d2, s2) = signed(&root(9), &with_updaters(2, &[&old, &new], &[]));
        let mut again = TrustState::load(Some(&d), root(9).verifying_key());
        assert!(again.accept(&d2, &s2, NOW).is_err());
        let (d3, s3) = signed(&root(9), &with_updaters(3, &[&new], &[]));
        again.accept(&d3, &s3, NOW).unwrap();
        assert_eq!(
            choose_updater_key(&old, again.statement(), &sig_by(&old_pair)),
            Err(UpdaterRefusal::Revoked),
            "serial 3 did not list the revocation, it still holds"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_floor_of_a_release_refuses_old_statements_and_revoked_keys() {
        let floor = Floor {
            min_serial: 3,
            revoked_bundle_keys: &["0123456789abcdef"],
            revoked_updater_keys: &[],
        };
        let fresh = || TrustState::load_with(None, root(9).verifying_key(), floor);
        // No stored statement: a replayed old one is not taken.
        let (d2, s2) = signed(&root(9), &doc(2, &[2], &[1]));
        assert!(
            fresh()
                .accept(&d2, &s2, NOW)
                .unwrap_err()
                .contains("this release")
        );
        let (d3, s3) = signed(&root(9), &doc(3, &[2], &[1]));
        assert_eq!(fresh().accept(&d3, &s3, NOW), Ok(Accepted::Updated));
        // A key the release dropped stays out of every statement and every list.
        let mut revoked_at_build = fresh();
        assert!(revoked_at_build.revoked_bundle.contains("0123456789abcdef"));
        revoked_at_build.revoked_bundle.insert(key_id_of(1));
        let (d4, s4) = signed(&root(9), &doc(4, &[1], &[]));
        assert!(revoked_at_build.accept(&d4, &s4, NOW).is_err());
        // Stored below the floor (an older release's file): ignored.
        let d = dir("floor");
        TrustState::load(Some(&d), root(9).verifying_key())
            .accept(&d2, &s2, NOW)
            .unwrap();
        let held = TrustState::load_with(Some(&d), root(9).verifying_key(), floor);
        assert!(held.statement().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn pair() -> (String, minisign::KeyPair) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        (STANDARD.encode(pair.pk.to_box().unwrap().to_string()), pair)
    }

    fn sig_by(pair: &minisign::KeyPair) -> String {
        STANDARD.encode(
            minisign::sign(Some(&pair.pk), &pair.sk, &b"installer"[..], Some("c"), None)
                .unwrap()
                .to_string(),
        )
    }

    #[test]
    fn minisign_key_ids_are_read_from_keys_and_signatures() {
        let (pubkey, pair) = pair();
        let id = minisign_key_id(&pubkey).unwrap();
        assert_eq!(
            signature_key_id(&sig_by(&pair)).as_deref(),
            Some(id.as_str())
        );
        assert_eq!(
            minisign_key_id(UPDATER).as_deref(),
            Some("2f6bbfae9c291e87")
        );
        assert!(minisign_key_id("%%%").is_none() && signature_key_id("").is_none());
    }

    fn updater_statement(allowed: &[&str], revoked: &[&str]) -> Statement {
        Statement {
            schema: 1,
            serial: 2,
            issued_at: "2026-10-17T10:00:00Z".into(),
            expires_at: "2027-01-15T10:00:00Z".into(),
            bundle_keys: vec![key_hex(1)],
            revoked_bundle_keys: vec![],
            updater_keys: allowed.iter().map(|k| (*k).to_owned()).collect(),
            revoked_updater_keys: revoked.iter().map(|k| (*k).to_owned()).collect(),
            max_version: BTreeMap::new(),
        }
    }

    #[test]
    fn updates_are_checked_against_the_updater_keys_of_the_statement() {
        let (old, old_pair) = pair();
        let (new, new_pair) = pair();
        let (old_sig, new_sig) = (sig_by(&old_pair), sig_by(&new_pair));
        let old_id = minisign_key_id(&old).unwrap();

        // No statement: the pinned key decides, whatever the signature says.
        assert_eq!(choose_updater_key(&old, None, &new_sig), Ok(old.clone()));

        // Listed and not revoked: accepted. A statement can introduce a key.
        let both = updater_statement(&[&old, &new], &[]);
        assert_eq!(
            choose_updater_key(&old, Some(&both), &old_sig),
            Ok(old.clone())
        );
        assert_eq!(
            choose_updater_key(&old, Some(&both), &new_sig),
            Ok(new.clone())
        );

        // Revoked: refused, even if it is the pinned one.
        let rotated = updater_statement(&[&new], &[&old_id]);
        assert_eq!(
            choose_updater_key(&old, Some(&rotated), &old_sig),
            Err(UpdaterRefusal::Revoked)
        );
        assert_eq!(
            choose_updater_key(&old, Some(&rotated), &new_sig),
            Ok(new.clone())
        );

        // Not listed: refused. Malformed signature: refused.
        let only_new = updater_statement(&[&new], &[]);
        assert_eq!(
            choose_updater_key(&old, Some(&only_new), &old_sig),
            Err(UpdaterRefusal::NotAllowed)
        );
        assert_eq!(
            choose_updater_key(&old, Some(&only_new), "garbage"),
            Err(UpdaterRefusal::Malformed)
        );

        // No updater keys listed: the pinned key stays, minus revocations.
        let none = updater_statement(&[], &[]);
        assert_eq!(
            choose_updater_key(&old, Some(&none), &old_sig),
            Ok(old.clone())
        );
        let revoked_only = updater_statement(&[], &[&old_id]);
        assert_eq!(
            choose_updater_key(&old, Some(&revoked_only), &old_sig),
            Err(UpdaterRefusal::Revoked)
        );
    }

    #[test]
    fn times_are_parsed_strictly_to_unix_seconds() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-11-01T00:00:00Z"), Some(NOW));
        assert_eq!(parse_utc("2024-02-29T12:30:45Z"), Some(1_709_209_845));
        assert_eq!(parse_utc("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert!(parse_utc("2026-04-30T00:00:00Z").is_some());
        for bad in [
            "2026-11-01 00:00:00Z",
            "2026-11-01T00:00:00+00:00",
            "2026-13-01T00:00:00Z",
            "2026-11-32T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-02-31T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2026-11-01T24:00:00Z",
            "2026-11-01T00:60:00Z",
            "2026-11-01T00:00:60Z",
            "2026-1x-01T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "",
        ] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_expired_statement_is_never_accepted_and_a_stored_one_stays_in_force_but_stale() {
        let (d, s) = signed(&root(9), &doc(1, &[1], &[]));
        let after = parse_utc("2027-01-15T10:00:00Z").unwrap();
        let mut trust = state(9);
        assert!(!trust.fresh(NOW), "no statement is not fresh");
        assert!(trust.accept(&d, &s, after).unwrap_err().contains("expired"));
        assert!(trust.accept(&d, &s, after - 1).is_ok());
        assert!(trust.fresh(after - 1));
        // Past expiry: still the statement in force (its revocations count),
        // but not fresh, and a replay of it is refused.
        assert!(!trust.fresh(after));
        assert_eq!(trust.statement().unwrap().serial, 1);
        assert!(trust.accept(&d, &s, after).is_err());
        let again = TrustState::load(None, root(9).verifying_key());
        assert!(!again.fresh(NOW));
    }

    #[test]
    fn caps_are_part_of_the_signed_statement_and_must_be_well_formed() {
        let with = |caps: serde_json::Value| {
            let mut v: serde_json::Value = serde_json::from_str(&doc(1, &[1], &[])).unwrap();
            v["max_version"] = caps;
            v.to_string()
        };
        let mut trust = state(9);
        for bad in [
            serde_json::json!({"x": "0.9.0"}),
            serde_json::json!({"0": "0.9.0"}),
            serde_json::json!({"1": "latest"}),
        ] {
            let (d, s) = signed(&root(9), &with(bad));
            assert!(trust.accept(&d, &s, NOW).is_err());
        }
        let (d, s) = signed(
            &root(9),
            &with(serde_json::json!({"1": "0.9.0", "2": "1.0.0"})),
        );
        trust.accept(&d, &s, NOW).unwrap();
        let caps = trust.statement().unwrap().caps();
        assert_eq!(caps[&1], Version(0, 9, 0));
        assert_eq!(caps[&2], Version(1, 0, 0));
        // Statements without the field have no caps.
        let (d, s) = signed(&root(9), &doc(2, &[1], &[]));
        trust.accept(&d, &s, NOW).unwrap();
        assert!(trust.statement().unwrap().caps().is_empty());
    }
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports this module to TypeScript (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::*;
    use serde_json::Value;

    fn load() -> Value {
        serde_json::from_str(include_str!("../../../test-vectors/trust.json"))
            .expect("the vector file is JSON")
    }

    fn state(root: &str) -> TrustState {
        TrustState::load(None, parse_key(root).unwrap().key)
    }

    fn accepted(result: Result<Accepted, String>) -> Value {
        match result {
            Ok(Accepted::Updated) => Value::String("updated".into()),
            Ok(Accepted::Unchanged) => Value::String("unchanged".into()),
            Err(message) => serde_json::json!({ "error": message }),
        }
    }

    #[test]
    fn times() {
        let v = load();
        for c in v["times"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            assert_eq!(parse_utc(input), c["secs"].as_u64(), "{input:?}");
        }
    }

    #[test]
    fn statements() {
        let v = load();
        let root = parse_key(v["root"].as_str().unwrap()).unwrap().key;
        for c in v["statements"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let doc = c["doc"].as_str().unwrap().as_bytes();
            let sig = c["sig"].as_str().unwrap().as_bytes();
            let verify = match verify_statement(doc, sig, &root) {
                Ok(s) => serde_json::json!({ "ok": {
                    "serial": s.serial,
                    "bundle_keys": s.bundle_keys().iter().map(|k| k.id.clone()).collect::<Vec<_>>(),
                    "caps": s.caps().iter().map(|(k, v)| (k.to_string(), Value::String(v.to_string()))).collect::<serde_json::Map<_, _>>(),
                    "updater_keys": s.updater_keys.len(),
                }}),
                Err(message) => serde_json::json!({ "error": message }),
            };
            assert_eq!(verify, c["verify"], "verify: {name}");
            let got =
                state(v["root"].as_str().unwrap()).accept(doc, sig, c["now"].as_u64().unwrap());
            assert_eq!(accepted(got), c["accept"], "accept: {name}");
        }
    }

    #[test]
    fn sequences() {
        let v = load();
        for seq in v["sequences"].as_array().unwrap() {
            let mut trust = state(v["root"].as_str().unwrap());
            for (i, s) in seq["steps"].as_array().unwrap().iter().enumerate() {
                let got = trust.accept(
                    s["doc"].as_str().unwrap().as_bytes(),
                    s["sig"].as_str().unwrap().as_bytes(),
                    s["now"].as_u64().unwrap(),
                );
                let at = format!("{} #{i}", seq["name"].as_str().unwrap());
                assert_eq!(accepted(got), s["expect"], "{at}");
                assert_eq!(
                    trust.statement().map(|s| s.serial),
                    s["serial"].as_u64(),
                    "{at} serial"
                );
            }
        }
    }

    #[test]
    fn minisign_ids_and_updater_choices() {
        let v = load();
        for c in v["minisign_ids"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            let got = match c["kind"].as_str().unwrap() {
                "key" => minisign_key_id(input),
                _ => signature_key_id(input),
            };
            assert_eq!(got.as_deref(), c["id"].as_str(), "{} {input:?}", c["kind"]);
        }
        for c in v["updater_choices"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let list = |key: &str| -> Vec<String> {
                c["statement"][key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|k| k.as_str().unwrap().to_owned())
                    .collect()
            };
            let statement = c["statement"].as_object().map(|_| Statement {
                schema: 1,
                serial: 2,
                issued_at: "x".into(),
                expires_at: "y".into(),
                bundle_keys: vec![],
                revoked_bundle_keys: vec![],
                updater_keys: list("updater_keys"),
                revoked_updater_keys: list("revoked_updater_keys"),
                max_version: BTreeMap::new(),
            });
            let got = choose_updater_key(
                c["pinned"].as_str().unwrap(),
                statement.as_ref(),
                c["signature"].as_str().unwrap(),
            );
            let got = match got {
                Ok(key) => serde_json::json!({ "key": key }),
                Err(UpdaterRefusal::Revoked) => serde_json::json!({ "refusal": "revoked" }),
                Err(UpdaterRefusal::NotAllowed) => serde_json::json!({ "refusal": "not_allowed" }),
                Err(UpdaterRefusal::Malformed) => serde_json::json!({ "refusal": "malformed" }),
            };
            assert_eq!(got, c["expect"], "{name}");
        }
    }
}
