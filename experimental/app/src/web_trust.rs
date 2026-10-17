//! Signed trust statement: remote revocation of the CI signing keys
//! (mnema-talk `doc/desktop-trusted-client.md`, "Key revocation").
//!
//! The client pins one offline root key. A statement signed by it names the
//! web bundle keys and Tauri updater keys that are allowed and the ones that
//! are revoked. Where the bytes come from decides nothing: the signature is
//! checked before anything is parsed, and a statement with a lower serial than
//! the highest one accepted is refused.
use crate::web_bundle::{TrustedKey, parse_key};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use std::{
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
    pub bundle_keys: Vec<String>,
    pub revoked_bundle_keys: Vec<String>,
    pub updater_keys: Vec<String>,
    pub revoked_updater_keys: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

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
        if self.bundle_keys.is_empty() {
            return Err("trust statement lists no bundle key".into());
        }
        if !self.revoked_bundle_keys.iter().all(|id| valid_id(id))
            || !self.revoked_updater_keys.iter().all(|id| valid_id(id))
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

/// The last good statement, persisted next to the bundle cache.
pub struct TrustState {
    dir: Option<PathBuf>,
    root: VerifyingKey,
    current: Option<(Statement, Vec<u8>)>,
}

impl TrustState {
    /// Loads and re-verifies the stored statement; a damaged or foreign file is
    /// ignored, which falls back to the compiled-in keys.
    pub fn load(dir: Option<&Path>, root: VerifyingKey) -> TrustState {
        let current = dir.and_then(|d| {
            let doc = std::fs::read(d.join(STATEMENT_FILE)).ok()?;
            let sig = std::fs::read(d.join(SIGNATURE_FILE)).ok()?;
            let statement = verify_statement(&doc, &sig, &root).ok()?;
            Some((statement, doc))
        });
        TrustState {
            dir: dir.map(Path::to_path_buf),
            root,
            current,
        }
    }

    pub fn statement(&self) -> Option<&Statement> {
        self.current.as_ref().map(|(s, _)| s)
    }

    /// Bundle keys in force: the statement's, else the compiled-in ones.
    pub fn effective_keys(&self, pinned: Vec<TrustedKey>) -> Vec<TrustedKey> {
        match self.statement() {
            Some(s) => s.bundle_keys(),
            None => pinned,
        }
    }

    /// Takes a fetched statement. Lower serials are refused; the same serial
    /// is only accepted with identical bytes.
    pub fn accept(&mut self, doc: &[u8], sig: &[u8]) -> Result<Accepted, String> {
        let statement = verify_statement(doc, sig, &self.root)?;
        if let Some((held, held_doc)) = &self.current {
            if statement.serial < held.serial {
                return Err(format!(
                    "trust statement {} is older than the {} already accepted",
                    statement.serial, held.serial
                ));
            }
            if statement.serial == held.serial {
                return if held_doc == doc {
                    Ok(Accepted::Unchanged)
                } else {
                    Err("a different trust statement with the same serial".into())
                };
            }
        }
        if let Some(dir) = &self.dir {
            std::fs::create_dir_all(dir).map_err(|_| "cannot create the trust directory")?;
            let write = |name: &str, bytes: &[u8]| {
                let partial = dir.join(format!("{name}.partial"));
                std::fs::write(&partial, bytes)
                    .and_then(|()| std::fs::rename(&partial, dir.join(name)))
            };
            // A torn pair fails verification at load and is ignored.
            write(SIGNATURE_FILE, sig)
                .and_then(|()| write(STATEMENT_FILE, doc))
                .map_err(|_| "cannot store the trust statement")?;
        }
        self.current = Some((statement, doc.to_vec()));
        Ok(Accepted::Updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_bundle::fixture::{signing_key, trusted};
    use ed25519_dalek::Signer;

    const UPDATER: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDg3MUUyOTlDQUVCRjZCMkYKUldRdmE3K3VuQ2tlaDdOajlZaG9vbENvRFBXQmtQd1pKUjZmK1hwRW8rSCt4b04rZzhwWVY3SkEK";

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
        assert_eq!(statement.serial, 1);
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
        assert_eq!(trust.accept(&d, &s), Ok(Accepted::Updated));
        assert_eq!(trust.statement().unwrap().serial, 1);
    }

    #[test]
    fn a_statement_signed_by_another_key_is_refused() {
        let (d, s) = signed(&root(8), &doc(1, &[1], &[]));
        let mut trust = state(9);
        assert!(trust.accept(&d, &s).is_err());
        assert!(trust.statement().is_none());
    }

    #[test]
    fn a_tampered_statement_or_signature_is_refused() {
        let (d, s) = signed(&root(9), &doc(1, &[1], &[]));
        let mut trust = state(9);
        let mut bad = d.clone();
        bad[10] ^= 1;
        assert!(trust.accept(&bad, &s).is_err());
        assert!(trust.accept(&d, b"AAAA").is_err());
        assert!(trust.accept(&d, b"").is_err());
        let mut bad_sig = s.clone();
        bad_sig[0] = if bad_sig[0] == b'A' { b'B' } else { b'A' };
        assert!(trust.accept(&d, &bad_sig).is_err());
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
        ] {
            let (d, s) = signed(&root(9), &text);
            assert!(trust.accept(&d, &s).is_err(), "{text}");
        }
    }

    #[test]
    fn an_older_serial_is_refused_and_an_equal_one_changes_nothing() {
        let mut trust = state(9);
        let (d2, s2) = signed(&root(9), &doc(2, &[2], &[1]));
        let (d1, s1) = signed(&root(9), &doc(1, &[1], &[]));
        assert_eq!(trust.accept(&d2, &s2), Ok(Accepted::Updated));
        assert!(trust.accept(&d1, &s1).unwrap_err().contains("older"));
        assert_eq!(trust.accept(&d2, &s2), Ok(Accepted::Unchanged));
        let (dx, sx) = signed(&root(9), &doc(2, &[3], &[]));
        assert!(trust.accept(&dx, &sx).is_err());
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
        trust.accept(&d, &s).unwrap();
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
        trust.accept(&doc_bytes, &sig).unwrap();
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
        std::fs::write(d.join(STATEMENT_FILE), edited).unwrap();
        assert!(
            TrustState::load(Some(&d), root(9).verifying_key())
                .statement()
                .is_none()
        );
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
            bundle_keys: vec![key_hex(1)],
            revoked_bundle_keys: vec![],
            updater_keys: allowed.iter().map(|k| (*k).to_owned()).collect(),
            revoked_updater_keys: revoked.iter().map(|k| (*k).to_owned()).collect(),
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
}
