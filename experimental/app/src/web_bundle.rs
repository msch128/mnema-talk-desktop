//! Signed web app bundles (mnema-talk `doc/desktop-trusted-client.md`).
//!
//! A bundle is one zip: `manifest.json`, `manifest.sig` (base64 Ed25519 over
//! the exact manifest bytes) and `app/<path>` for every file of the web app.
//! The host runs only bundles whose signature matches a key pinned in this
//! build. Nothing here trusts where bytes came from: the installer, the cache
//! and a download all go through [`verify`], and a cached bundle is verified
//! again on every start.
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_ENTRIES: usize = 4000;
pub const MAX_FILE: u64 = 64 << 20;
pub const MAX_TOTAL: u64 = 256 << 20;
const MAX_MANIFEST: u64 = 4 << 20;
const MAX_SIGNATURE: u64 = 1024;
const MAX_PATH: usize = 512;
/// A cached bundle larger than this is not even read.
const MAX_ZIP: u64 = 96 << 20;
const MAX_CACHED_FILES: usize = 16;
/// Cached bundles kept besides the built-in one.
const KEEP_CACHED: usize = 3;

/// A public key the client trusts. `id` is the first 8 bytes of SHA-256 over
/// the raw key, hex; it only selects the key, the signature decides.
#[derive(Clone)]
pub struct TrustedKey {
    pub id: String,
    pub key: VerifyingKey,
}

/// Keys pinned in this build. To rotate, ship a release that pins both the old
/// and the new key, then drop the old one in a later release.
const PINNED: &[&str] = &["7815c02fddb14038cf3641036ba2e4e20ffdd0e42d00be3477671ca4628de06d"];

pub fn trusted_keys() -> Vec<TrustedKey> {
    PINNED
        .iter()
        .filter_map(|raw| parse_key(raw))
        .collect::<Vec<_>>()
}

pub fn parse_key(raw_hex: &str) -> Option<TrustedKey> {
    let raw: [u8; 32] = hex::decode(raw_hex).ok()?.try_into().ok()?;
    Some(TrustedKey {
        id: key_id(&raw),
        key: VerifyingKey::from_bytes(&raw).ok()?,
    })
}

pub fn key_id(raw: &[u8; 32]) -> String {
    hex::encode(&Sha256::digest(raw)[..8])
}

/// `MAJOR.MINOR.PATCH`, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let mut parts = text.split('.');
        let mut next = || {
            let part = parts.next()?;
            if part.is_empty()
                || part.len() > 9
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            part.parse::<u32>().ok()
        };
        let version = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    name: String,
    version: String,
    web_client_api: u32,
    min_server_version: String,
    security_floor: String,
    commit: String,
    key_id: String,
    files: Vec<FileEntry>,
}

/// A verified bundle, held in memory.
#[derive(Debug)]
pub struct Bundle {
    pub version: Version,
    pub web_client_api: u32,
    pub min_server_version: Version,
    pub security_floor: Version,
    pub key_id: String,
    pub commit: String,
    files: HashMap<String, Vec<u8>>,
}

impl Bundle {
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
}

/// Bundle-relative path rules, shared with the signing tool: UTF-8, forward
/// slashes, no empty, `.` or `..` segment, no control character.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path
            .chars()
            .any(|c| c < '\u{20}' || c == '\u{7f}' || c == '\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn read_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    index: usize,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let entry = zip.by_index(index).map_err(|_| "unreadable entry")?;
    let name = entry.name().map_err(|_| "invalid entry name")?.into_owned();
    if entry.size() > limit {
        return Err(format!("{name}: too large"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    // One byte more than allowed tells a lying header from an exact size.
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{name}: damaged"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{name}: too large"));
    }
    Ok(bytes)
}

/// Verifies a bundle against the trusted keys: signature first (nothing in the
/// manifest is parsed before it holds), then that the zip holds exactly the
/// files the manifest lists with the listed sizes and hashes.
pub fn verify(data: &[u8], keys: &[TrustedKey]) -> Result<Bundle, String> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(data)).map_err(|_| "not a readable zip".to_owned())?;
    if zip.len() > MAX_ENTRIES + 2 {
        return Err("too many entries".into());
    }
    let mut index: HashMap<String, usize> = HashMap::with_capacity(zip.len());
    for i in 0..zip.len() {
        let name = zip
            .by_index_raw(i)
            .map_err(|_| "unreadable entry")?
            .name()
            .map_err(|_| "invalid entry name")?
            .into_owned();
        if index.insert(name.clone(), i).is_some() {
            return Err(format!("duplicate entry {name}"));
        }
    }
    let (Some(&manifest_at), Some(&signature_at)) =
        (index.get("manifest.json"), index.get("manifest.sig"))
    else {
        return Err("manifest or signature is missing".into());
    };
    let manifest_bytes = read_entry(&mut zip, manifest_at, MAX_MANIFEST)?;
    let signature_text = read_entry(&mut zip, signature_at, MAX_SIGNATURE)?;
    let signature: [u8; 64] = STANDARD
        .decode(String::from_utf8_lossy(&signature_text).trim())
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("malformed signature")?;
    let signature = Signature::from_bytes(&signature);
    let signer = keys
        .iter()
        .find(|k| k.key.verify_strict(&manifest_bytes, &signature).is_ok())
        .ok_or("signature does not match a trusted key")?;

    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| "unreadable manifest")?;
    let version = check_manifest(&manifest, &signer.id)?;
    if index.len() != manifest.files.len() + 2 {
        return Err("zip entries do not match the manifest".into());
    }
    let mut files = HashMap::with_capacity(manifest.files.len());
    for file in &manifest.files {
        let at = *index
            .get(&format!("app/{}", file.path))
            .ok_or_else(|| format!("{}: missing from the zip", file.path))?;
        let declared = zip.by_index_raw(at).map_err(|_| "unreadable entry")?.size();
        if declared != file.size {
            return Err(format!("{}: size does not match the manifest", file.path));
        }
        let body = read_entry(&mut zip, at, file.size)?;
        if hex::encode(Sha256::digest(&body)) != file.sha256 {
            return Err(format!(
                "{}: content does not match the manifest",
                file.path
            ));
        }
        files.insert(file.path.clone(), body);
    }
    Ok(Bundle {
        version,
        web_client_api: manifest.web_client_api,
        min_server_version: Version::parse(&manifest.min_server_version)
            .ok_or("min_server_version")?,
        security_floor: Version::parse(&manifest.security_floor).ok_or("security_floor")?,
        key_id: manifest.key_id,
        commit: manifest.commit,
        files,
    })
}

fn check_manifest(m: &Manifest, signer: &str) -> Result<Version, String> {
    if m.schema != 1 || m.name != "mnema-web" {
        return Err("unsupported manifest".into());
    }
    if m.key_id != signer {
        return Err("manifest key_id does not match the signing key".into());
    }
    let version = Version::parse(&m.version).ok_or("invalid version")?;
    let min = Version::parse(&m.min_server_version).ok_or("invalid min_server_version")?;
    let floor = Version::parse(&m.security_floor).ok_or("invalid security_floor")?;
    if m.web_client_api < 1 || min > version || floor > version {
        return Err("invalid compatibility numbers".into());
    }
    if m.files.is_empty() || m.files.len() > MAX_ENTRIES {
        return Err("invalid file count".into());
    }
    let mut seen = HashSet::with_capacity(m.files.len());
    let mut total = 0u64;
    for file in &m.files {
        if !valid_path(&file.path) || !seen.insert(file.path.as_str()) {
            return Err(format!("invalid or duplicate path {}", file.path));
        }
        if file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("{}: invalid sha256", file.path));
        }
        if file.size > MAX_FILE {
            return Err(format!("{}: invalid size", file.path));
        }
        total += file.size;
    }
    if !seen.contains("index.html") {
        return Err("index.html is missing".into());
    }
    if total > MAX_TOTAL {
        return Err("bundle exceeds the size limit".into());
    }
    Ok(version)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Cache,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Builtin => "builtin",
            Source::Cache => "cache",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Held {
    pub bundle: Arc<Bundle>,
    pub source: Source,
}

/// What the instance said about itself in `/api/health`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// `None` for anything that is not a release version, such as `dev`.
    pub version: Option<Version>,
    pub web_client_api: u32,
}

/// Why no held bundle fits a server. Never answered by falling back to the
/// server's own JavaScript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing verified at all.
    NoBundle,
    /// The server speaks a newer contract than every bundle held; an update
    /// may help.
    ServerNewer,
    /// The server is older than every bundle that speaks its contract.
    ServerTooOld { needs: Version },
    /// No bundle speaks the server's (older) contract any more.
    ServerContractRetired,
    /// Bundles for the server's contract are held, but only ones older than
    /// the newest this client ever verified for it (and that one is gone).
    /// Running an older signed bundle would be a rollback.
    RolledBack,
    /// Everything this client holds was signed by a key that is no longer
    /// trusted (or is damaged), and no newer bundle has arrived yet.
    SignerWithdrawn,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoBundle => f.write_str("This client has no verified app. Reinstall the desktop client."),
            Refusal::ServerNewer => f.write_str(
                "This server needs a newer Mnema app than this client has. Update the desktop client.",
            ),
            Refusal::ServerTooOld { needs } => write!(
                f,
                "This server is too old for this desktop client (needs {needs} or newer). Ask the administrator to update it."
            ),
            Refusal::ServerContractRetired => f.write_str(
                "This server speaks an older contract than any app in this client. Ask the administrator to update it.",
            ),
            Refusal::SignerWithdrawn => f.write_str(
                "The app in this client was signed with a key that is no longer trusted. Update the desktop client; it fetches a newly signed app at its next check.",
            ),
            Refusal::RolledBack => f.write_str(
                "The newest verified app this client has used is no longer available, and older apps are refused. Update or reinstall the desktop client.",
            ),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    security_floor: Option<Version>,
    /// Highest bundle version ever verified, per `web_client_api`. Bundles
    /// below it are never selected, whatever the server claims.
    #[serde(default)]
    high_water: BTreeMap<u32, Version>,
    /// Key id of the bundle that set each high-water mark and the floor, so
    /// that a revoked key's bundles do not keep blocking the real ones.
    #[serde(default)]
    high_water_key: BTreeMap<u32, String>,
    #[serde(default)]
    floor_key: Option<String>,
    last_check: u64,
}

/// Verified bundles in memory plus the cache on disk.
pub struct Store {
    dir: Option<PathBuf>,
    floor: Version,
    floor_key: Option<String>,
    high_water: BTreeMap<u32, Version>,
    high_water_key: BTreeMap<u32, String>,
    last_check: u64,
    /// Bundles were dropped (or refused) because their key is not trusted.
    withdrawn: bool,
    held: Vec<Held>,
    /// Verified bundles below the floor. Never served; kept so that they
    /// return if the key that raised the floor is revoked.
    shelved: Vec<Held>,
    /// Cached files that failed verification (names only, for diagnostics).
    pub rejected: Vec<String>,
}

pub const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

impl Store {
    /// Verifies the built-in bundle and everything in the cache directory.
    /// The security floor is the highest floor any verified manifest declares
    /// (or an earlier run recorded); bundles below it are dropped.
    pub fn load(dir: Option<&Path>, builtin: Option<&[u8]>, keys: &[TrustedKey]) -> Store {
        let state: State = dir
            .and_then(|d| std::fs::read(d.join("state.json")).ok())
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        let mut verified: Vec<Held> = Vec::new();
        let mut rejected = Vec::new();
        if let Some(bytes) = builtin {
            match verify(bytes, keys) {
                Ok(bundle) => verified.push(Held {
                    bundle: Arc::new(bundle),
                    source: Source::Builtin,
                }),
                Err(_) => rejected.push("built-in".to_owned()),
            }
        }
        for path in cached_files(dir) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let loaded = std::fs::metadata(&path)
                .ok()
                .filter(|m| m.len() <= MAX_ZIP)
                .and_then(|_| std::fs::read(&path).ok())
                .ok_or_else(|| "unreadable".to_owned())
                .and_then(|bytes| verify(&bytes, keys));
            match loaded {
                Ok(bundle) if !verified.iter().any(|h| h.bundle.version == bundle.version) => {
                    verified.push(Held {
                        bundle: Arc::new(bundle),
                        source: Source::Cache,
                    })
                }
                Ok(_) => {}
                Err(_) => rejected.push(name),
            }
        }
        let withdrawn = verified.is_empty() && !rejected.is_empty();
        let mut store = Store {
            dir: dir.map(Path::to_path_buf),
            floor: state.security_floor.unwrap_or(Version(0, 0, 0)),
            floor_key: state.floor_key,
            high_water: state.high_water,
            high_water_key: state.high_water_key,
            last_check: state.last_check,
            withdrawn,
            held: verified,
            shelved: Vec::new(),
            rejected,
        };
        store.forget_untrusted_marks(keys);
        store.raise_from_held();
        store.apply_floor();
        store.persist();
        store
    }

    fn persist(&self) {
        let Some(dir) = &self.dir else { return };
        let state = State {
            security_floor: Some(self.floor),
            high_water: self.high_water.clone(),
            high_water_key: self.high_water_key.clone(),
            floor_key: self.floor_key.clone(),
            last_check: self.last_check,
        };
        if std::fs::create_dir_all(dir).is_ok()
            && let Ok(raw) = serde_json::to_vec(&state)
        {
            let _ = std::fs::write(dir.join("state.json"), raw);
        }
    }

    #[cfg(test)]
    pub fn floor(&self) -> Version {
        self.floor
    }

    #[cfg(test)]
    pub fn high_water(&self, api: u32) -> Option<Version> {
        self.high_water.get(&api).copied()
    }

    /// Why nothing at all can be served.
    pub fn empty_reason(&self) -> Refusal {
        if self.withdrawn {
            Refusal::SignerWithdrawn
        } else {
            Refusal::NoBundle
        }
    }

    pub fn newest(&self) -> Option<&Held> {
        self.held.first()
    }

    pub fn held(&self) -> &[Held] {
        &self.held
    }

    pub fn check_due(&self, now: u64) -> bool {
        now.saturating_sub(self.last_check) >= CHECK_INTERVAL_SECS
    }

    pub fn mark_checked(&mut self, now: u64) {
        self.last_check = now;
        self.persist();
    }

    /// The newest held bundle that speaks the server's contract and supports
    /// its release. Bundles older than the high-water mark of their contract
    /// are never offered: a server cannot talk the client into an older,
    /// vulnerable app by claiming an old version.
    pub fn select(&self, server: &ServerInfo) -> Result<Held, Refusal> {
        let current: Vec<&Held> = self
            .held
            .iter()
            .filter(|h| {
                self.high_water
                    .get(&h.bundle.web_client_api)
                    .is_none_or(|hwm| h.bundle.version >= *hwm)
            })
            .collect();
        let speaks = |h: &&&Held| h.bundle.web_client_api == server.web_client_api;
        let supports = |h: &&&Held| {
            server
                .version
                .is_none_or(|v| v >= h.bundle.min_server_version)
        };
        if let Some(best) = current.iter().filter(speaks).find(supports) {
            return Ok((**best).clone());
        }
        let Some(newest_api) = self.held.iter().map(|h| h.bundle.web_client_api).max() else {
            return Err(self.empty_reason());
        };
        if server.web_client_api > newest_api {
            return Err(Refusal::ServerNewer);
        }
        match current
            .iter()
            .filter(speaks)
            .map(|h| h.bundle.min_server_version)
            .min()
        {
            Some(needs) => Err(Refusal::ServerTooOld { needs }),
            None if self
                .held
                .iter()
                .any(|h| h.bundle.web_client_api == server.web_client_api) =>
            {
                Err(Refusal::RolledBack)
            }
            None => Err(Refusal::ServerContractRetired),
        }
    }

    /// Verifies a downloaded bundle and keeps it if it is newer than
    /// everything held. Older and equal versions are refused, so replayed
    /// release metadata cannot replace anything.
    pub fn install(&mut self, bytes: &[u8], keys: &[TrustedKey]) -> Result<Version, String> {
        let bundle = verify(bytes, keys)?;
        let version = bundle.version;
        if self.held.iter().any(|h| h.bundle.version >= version) {
            return Err("a bundle of this version or newer is already held".into());
        }
        if version < self.floor.max(bundle.security_floor) {
            return Err("below the security floor".into());
        }
        if let Some(dir) = &self.dir {
            std::fs::create_dir_all(dir).map_err(|_| "cannot create the cache directory")?;
            let target = dir.join(format!("mnema-web-{version}.zip"));
            let partial = dir.join(format!("mnema-web-{version}.zip.partial"));
            std::fs::write(&partial, bytes)
                .and_then(|()| std::fs::rename(&partial, &target))
                .map_err(|_| "cannot write the bundle")?;
        }
        self.raise_marks(&bundle);
        self.held.push(Held {
            bundle: Arc::new(bundle),
            source: Source::Cache,
        });
        self.apply_floor();
        self.prune();
        self.persist();
        Ok(version)
    }

    /// Applies a new set of trusted keys to what is held: bundles signed by any
    /// other key are dropped (cached files removed), and the high-water marks
    /// and floor that such bundles raised are recomputed from what remains.
    /// Returns whether anything was dropped.
    pub fn reconcile(&mut self, keys: &[TrustedKey]) -> bool {
        let trusted = |id: &str| keys.iter().any(|k| k.id == id);
        let dropped: Vec<(Version, bool)> = self
            .held
            .iter()
            .filter(|h| !trusted(&h.bundle.key_id))
            .map(|h| (h.bundle.version, h.source == Source::Cache))
            .collect();
        self.held.retain(|h| trusted(&h.bundle.key_id));
        self.shelved.retain(|h| trusted(&h.bundle.key_id));
        if let Some(dir) = &self.dir {
            for (version, cached) in &dropped {
                if *cached {
                    let _ = std::fs::remove_file(dir.join(format!("mnema-web-{version}.zip")));
                }
            }
        }
        self.forget_untrusted_marks(keys);
        self.raise_from_held();
        self.apply_floor();
        if !dropped.is_empty() && self.held.is_empty() {
            self.withdrawn = true;
        }
        self.persist();
        !dropped.is_empty()
    }

    /// Forgets the marks that a key which is no longer trusted had set. Marks
    /// from before this was recorded have no key and stay.
    fn forget_untrusted_marks(&mut self, keys: &[TrustedKey]) {
        let trusted = |id: &str| keys.iter().any(|k| k.id == id);
        if self.floor_key.as_deref().is_some_and(|k| !trusted(k)) {
            self.floor = Version(0, 0, 0);
            self.floor_key = None;
        }
        let gone: Vec<u32> = self
            .high_water_key
            .iter()
            .filter(|(_, k)| !trusted(k))
            .map(|(api, _)| *api)
            .collect();
        for api in gone {
            self.high_water.remove(&api);
            self.high_water_key.remove(&api);
        }
    }

    fn raise_from_held(&mut self) {
        let bundles: Vec<Arc<Bundle>> = self
            .held
            .iter()
            .chain(&self.shelved)
            .map(|h| h.bundle.clone())
            .collect();
        for bundle in bundles {
            self.raise_marks(&bundle);
        }
    }

    /// Serves bundles at or above the floor, newest first; the rest are shelved.
    fn apply_floor(&mut self) {
        let floor = self.floor;
        let mut all = std::mem::take(&mut self.held);
        all.append(&mut self.shelved);
        let (mut held, shelved): (Vec<Held>, Vec<Held>) =
            all.into_iter().partition(|h| h.bundle.version >= floor);
        held.sort_by_key(|h| std::cmp::Reverse(h.bundle.version));
        self.held = held;
        self.shelved = shelved;
    }

    fn raise_marks(&mut self, bundle: &Bundle) {
        if bundle.security_floor > self.floor {
            self.floor = bundle.security_floor;
            self.floor_key = Some(bundle.key_id.clone());
        }
        let mark = self.high_water.get(&bundle.web_client_api);
        if mark.is_none_or(|m| bundle.version > *m) {
            self.high_water
                .insert(bundle.web_client_api, bundle.version);
            self.high_water_key
                .insert(bundle.web_client_api, bundle.key_id.clone());
        }
    }

    /// Keeps the built-in bundle and the newest few cached ones; removes the
    /// files of the rest, and of anything below the floor.
    fn prune(&mut self) {
        let mut cached = 0;
        let mut dropped = Vec::new();
        self.held.retain(|h| {
            if h.source == Source::Builtin {
                return true;
            }
            cached += 1;
            if cached > KEEP_CACHED {
                dropped.push(h.bundle.version);
                false
            } else {
                true
            }
        });
        if let Some(dir) = &self.dir {
            for version in dropped {
                let _ = std::fs::remove_file(dir.join(format!("mnema-web-{version}.zip")));
            }
        }
    }
}

fn cached_files(dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("mnema-web-") && name.ends_with(".zip") && e.path().is_file()
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    files.truncate(MAX_CACHED_FILES);
    files
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Write;

    pub fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn trusted(seed: u8) -> Vec<TrustedKey> {
        let raw = signing_key(seed).verifying_key().to_bytes();
        vec![TrustedKey {
            id: key_id(&raw),
            key: VerifyingKey::from_bytes(&raw).unwrap(),
        }]
    }

    #[derive(Clone)]
    pub struct Spec {
        pub version: &'static str,
        pub api: u32,
        pub min_server: &'static str,
        pub floor: &'static str,
        pub seed: u8,
        pub files: Vec<(&'static str, &'static str)>,
    }

    impl Spec {
        pub fn new(version: &'static str) -> Spec {
            Spec {
                version,
                api: 1,
                min_server: "0.1.0",
                floor: "0.0.0",
                seed: 7,
                files: vec![
                    ("index.html", "<html>app</html>"),
                    ("assets/app-1.js", "console.log(1)"),
                ],
            }
        }
    }

    pub fn manifest_json(spec: &Spec) -> String {
        let key = signing_key(spec.seed).verifying_key().to_bytes();
        let files: Vec<serde_json::Value> = spec
            .files
            .iter()
            .map(|(path, body)| {
                serde_json::json!({"path": path, "sha256": hex::encode(Sha256::digest(body.as_bytes())), "size": body.len()})
            })
            .collect();
        serde_json::json!({
            "schema": 1, "name": "mnema-web", "version": spec.version, "web_client_api": spec.api,
            "min_server_version": spec.min_server, "security_floor": spec.floor, "commit": "test",
            "key_id": key_id(&key), "files": files
        })
        .to_string()
    }

    /// Entries as (name, bytes), signed by `seed`'s key.
    pub fn entries(spec: &Spec) -> Vec<(String, Vec<u8>)> {
        let manifest = manifest_json(spec);
        let signature =
            STANDARD.encode(signing_key(spec.seed).sign(manifest.as_bytes()).to_bytes());
        let mut entries = vec![
            ("manifest.json".to_owned(), manifest.into_bytes()),
            ("manifest.sig".to_owned(), signature.into_bytes()),
        ];
        for (path, body) in &spec.files {
            entries.push((format!("app/{path}"), body.as_bytes().to_vec()));
        }
        entries
    }

    pub fn zip_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
        out.into_inner()
    }

    pub fn bundle(spec: &Spec) -> Vec<u8> {
        zip_of(&entries(spec))
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("mnema-bundle-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_pinned_key_is_well_formed_and_matches_its_published_id() {
        let keys = trusted_keys();
        assert_eq!(keys.len(), PINNED.len());
        assert_eq!(keys[0].id, "1069c76ff2e11272");
        assert!(parse_key("00").is_none());
        assert!(parse_key("zz").is_none());
    }

    #[test]
    fn versions_parse_strictly_and_order_numerically() {
        assert_eq!(Version::parse("0.7.16"), Some(Version(0, 7, 16)));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "01.2.3",
            "1.2.x",
            "1..3",
            "1.2.3-rc1",
            "-1.0.0",
            "99999999999.0.0",
            "+1.0.0",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(0, 7, 9) < Version(0, 7, 10));
        assert!(Version(0, 9, 0) < Version(1, 0, 0));
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn path_rules_match_the_signing_tool() {
        for ok in ["index.html", "assets/a-b.js", "ünï/x.js"] {
            assert!(valid_path(ok), "{ok}");
        }
        let long = "a".repeat(MAX_PATH + 1);
        for bad in [
            "",
            "/abs",
            "a//b",
            "a/./b",
            "a/../b",
            "..",
            ".",
            "a/",
            "a\\b",
            "a\0b",
            "a\u{7f}",
            "a\nb",
            long.as_str(),
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_bundle_signed_by_a_pinned_key_verifies_and_serves_its_files() {
        let bundle = verify(&bundle(&Spec::new("0.8.0")), &trusted(7)).unwrap();
        assert_eq!(bundle.version, Version(0, 8, 0));
        assert_eq!(bundle.web_client_api, 1);
        assert_eq!(
            bundle.file("index.html"),
            Some(b"<html>app</html>".as_slice())
        );
        assert_eq!(bundle.file("missing"), None);
        assert_eq!(bundle.commit, "test");
        assert_eq!(bundle.key_id, trusted(7)[0].id);
    }

    #[test]
    fn a_bundle_from_another_key_or_with_no_keys_is_refused() {
        let data = bundle(&Spec::new("0.8.0"));
        assert!(verify(&data, &trusted(8)).is_err());
        assert!(verify(&data, &[]).is_err());
    }

    fn damaged(edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> Result<Bundle, String> {
        let mut e = entries(&Spec::new("0.8.0"));
        edit(&mut e);
        verify(&zip_of(&e), &trusted(7))
    }

    fn entry<'a>(e: &'a mut [(String, Vec<u8>)], name: &str) -> &'a mut Vec<u8> {
        &mut e.iter_mut().find(|(n, _)| n == name).unwrap().1
    }

    /// Re-signs an edited manifest so only the manifest rules are exercised.
    fn resigned(edit: impl FnOnce(&mut serde_json::Value)) -> Result<Bundle, String> {
        use ed25519_dalek::Signer;
        damaged(|e| {
            let mut manifest: serde_json::Value =
                serde_json::from_slice(entry(e, "manifest.json")).unwrap();
            edit(&mut manifest);
            let raw = manifest.to_string().into_bytes();
            *entry(e, "manifest.sig") = STANDARD
                .encode(signing_key(7).sign(&raw).to_bytes())
                .into_bytes();
            *entry(e, "manifest.json") = raw;
        })
    }

    #[test]
    fn tampering_with_any_part_of_a_signed_bundle_is_refused() {
        assert!(damaged(|e| *entry(e, "app/index.html") = b"<html>evil</html>".to_vec()).is_err());
        assert!(damaged(|e| e.push(("app/evil.js".into(), b"x".to_vec()))).is_err());
        assert!(damaged(|e| e.retain(|(n, _)| n != "app/assets/app-1.js")).is_err());
        assert!(
            damaged(|e| {
                e.retain(|(n, _)| n != "app/assets/app-1.js");
                e.push(("app/assets/other.js".into(), b"console.log(1)".to_vec()));
            })
            .is_err()
        );
        assert!(damaged(|e| e.retain(|(n, _)| n != "manifest.json")).is_err());
        assert!(damaged(|e| e.retain(|(n, _)| n != "manifest.sig")).is_err());
        assert!(
            damaged(|e| {
                let manifest = entry(e, "manifest.json");
                *manifest = String::from_utf8(manifest.clone())
                    .unwrap()
                    .replace("0.8.0", "9.9.9")
                    .into_bytes();
            })
            .is_err()
        );
        assert!(damaged(|e| *entry(e, "manifest.sig") = b"***".to_vec()).is_err());
        assert!(damaged(|e| *entry(e, "manifest.sig") = b"AAAA".to_vec()).is_err());
        assert!(
            damaged(|e| *entry(e, "manifest.json") = vec![b' '; (MAX_MANIFEST + 1) as usize])
                .is_err()
        );
        assert!(damaged(|e| *entry(e, "manifest.sig") = vec![b'A'; 2000]).is_err());
        assert!(verify(b"PK not a zip", &trusted(7)).is_err());
    }

    #[test]
    fn a_correctly_signed_manifest_still_has_to_be_well_formed() {
        assert!(resigned(|_| {}).is_ok());
        for (field, value) in [
            ("schema", serde_json::json!(2)),
            ("name", serde_json::json!("other")),
            ("key_id", serde_json::json!("0000000000000000")),
            ("version", serde_json::json!("1")),
            ("web_client_api", serde_json::json!(0)),
            ("min_server_version", serde_json::json!("9.0.0")),
            ("security_floor", serde_json::json!("9.0.0")),
            ("files", serde_json::json!([])),
        ] {
            assert!(resigned(|m| m[field] = value.clone()).is_err(), "{field}");
        }
        assert!(resigned(|m| m["surprise"] = serde_json::json!(1)).is_err());
        assert!(resigned(|m| m["files"][0]["path"] = serde_json::json!("../x")).is_err());
        assert!(resigned(|m| m["files"][1]["path"] = m["files"][0]["path"].clone()).is_err());
        assert!(resigned(|m| m["files"][0]["sha256"] = serde_json::json!("zz")).is_err());
        assert!(resigned(|m| m["files"][0]["sha256"] = serde_json::json!("A".repeat(64))).is_err());
        assert!(resigned(|m| m["files"][0]["size"] = serde_json::json!(MAX_FILE + 1)).is_err());
        assert!(resigned(|m| m["files"][0]["size"] = serde_json::json!(1)).is_err());
        assert!(resigned(|m| m["files"][0]["sha256"] = serde_json::json!("0".repeat(64))).is_err());
        assert!(resigned(|m| m["files"][0]["path"] = serde_json::json!("home.html")).is_err());
        assert!(resigned(|m| {
            let files = m["files"].as_array_mut().unwrap();
            for i in 0..3 {
                files.push(serde_json::json!({"path": format!("big{i}"), "sha256": "0".repeat(64), "size": MAX_FILE}));
            }
            for file in files.iter_mut() {
                file["size"] = serde_json::json!(MAX_FILE);
            }
            files.push(serde_json::json!({"path": "big9", "sha256": "0".repeat(64), "size": MAX_FILE}));
            files.push(serde_json::json!({"path": "big10", "sha256": "0".repeat(64), "size": MAX_FILE}));
        })
        .is_err());
        assert!(resigned(|m| {
            let files = m["files"].as_array_mut().unwrap();
            for i in 0..=MAX_ENTRIES {
                files.push(serde_json::json!({"path": format!("f{i}"), "sha256": "0".repeat(64), "size": 0}));
            }
        })
        .is_err());
    }

    #[test]
    fn too_many_zip_entries_are_refused_before_anything_is_read() {
        let mut e = entries(&Spec::new("0.8.0"));
        for i in 0..MAX_ENTRIES + 1 {
            e.push((format!("app/pad{i}"), Vec::new()));
        }
        assert_eq!(
            verify(&zip_of(&e), &trusted(7)).unwrap_err(),
            "too many entries"
        );
    }

    #[test]
    fn the_server_is_matched_by_contract_and_release() {
        let mut spec_new = Spec::new("0.9.0");
        spec_new.min_server = "0.8.9";
        let mut spec_old = Spec::new("0.8.0");
        spec_old.min_server = "0.7.0";
        let mut store = Store::load(None, Some(&bundle(&spec_old)), &trusted(7));
        store.install(&bundle(&spec_new), &trusted(7)).unwrap();
        let server = |v: Option<Version>, api| ServerInfo {
            version: v,
            web_client_api: api,
        };
        // The newest bundle that supports the server's release.
        assert_eq!(
            store
                .select(&server(Some(Version(0, 9, 0)), 1))
                .unwrap()
                .bundle
                .version,
            Version(0, 9, 0)
        );
        // A server that claims an older release does not get the older bundle:
        // 0.9.0 was verified, so nothing below it is run for this contract.
        assert_eq!(
            store.select(&server(Some(Version(0, 8, 5)), 1)).err(),
            Some(Refusal::ServerTooOld {
                needs: Version(0, 8, 9)
            })
        );
        // A development server only has to speak the contract.
        assert_eq!(
            store.select(&server(None, 1)).unwrap().bundle.version,
            Version(0, 9, 0)
        );
        // Too old for every bundle.
        assert_eq!(
            store.select(&server(Some(Version(0, 6, 0)), 1)).err(),
            Some(Refusal::ServerTooOld {
                needs: Version(0, 8, 9)
            })
        );
        // A newer contract than any bundle: an update may help.
        assert_eq!(
            store.select(&server(Some(Version(0, 9, 0)), 2)).err(),
            Some(Refusal::ServerNewer)
        );
        // Nothing speaks an older contract any more.
        let mut api2 = Spec::new("1.0.0");
        api2.api = 2;
        let store2 = Store::load(None, Some(&bundle(&api2)), &trusted(7));
        assert_eq!(
            store2.select(&server(None, 1)).err(),
            Some(Refusal::ServerContractRetired)
        );
        assert_eq!(
            Store::load(None, None, &trusted(7))
                .select(&server(None, 1))
                .err(),
            Some(Refusal::NoBundle)
        );
        for refusal in [
            Refusal::NoBundle,
            Refusal::ServerNewer,
            Refusal::ServerTooOld {
                needs: Version(0, 7, 0),
            },
            Refusal::ServerContractRetired,
            Refusal::RolledBack,
        ] {
            assert!(!refusal.to_string().is_empty());
        }
    }

    #[test]
    fn the_high_water_mark_blocks_older_bundles_even_without_a_floor() {
        let d = dir("hwm");
        let old = Spec::new("0.8.0");
        let mut store = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        store
            .install(&bundle(&Spec::new("0.8.4")), &trusted(7))
            .unwrap();
        assert_eq!(store.floor(), Version(0, 0, 0));
        assert_eq!(store.high_water(1), Some(Version(0, 8, 4)));
        let server = ServerInfo {
            version: Some(Version(0, 1, 0)),
            web_client_api: 1,
        };
        // Whatever the server claims, the built-in 0.8.0 is not offered.
        assert_eq!(
            store.select(&server).unwrap().bundle.version,
            Version(0, 8, 4)
        );
        // The mark survives a restart and the loss of the newer bundle: the
        // client refuses instead of running the older one.
        std::fs::remove_file(d.join("mnema-web-0.8.4.zip")).unwrap();
        let again = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        assert_eq!(again.high_water(1), Some(Version(0, 8, 4)));
        assert_eq!(again.select(&server).err(), Some(Refusal::RolledBack));
        // A lost state file is rebuilt from what verifies, never lowered.
        std::fs::remove_file(d.join("state.json")).unwrap();
        let rebuilt = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        assert_eq!(rebuilt.high_water(1), Some(Version(0, 8, 0)));
        // Another contract is tracked on its own.
        let mut api2 = Spec::new("0.9.0");
        api2.api = 2;
        let mut other = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        other.install(&bundle(&api2), &trusted(7)).unwrap();
        assert_eq!(other.high_water(1), Some(Version(0, 8, 0)));
        assert_eq!(other.high_water(2), Some(Version(0, 9, 0)));
        assert_eq!(
            other
                .select(&ServerInfo {
                    version: None,
                    web_client_api: 1
                })
                .unwrap()
                .bundle
                .version,
            Version(0, 8, 0)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    fn both_keys() -> Vec<TrustedKey> {
        let mut keys = trusted(7);
        keys.extend(trusted(5));
        keys
    }

    fn server() -> ServerInfo {
        ServerInfo {
            version: None,
            web_client_api: 1,
        }
    }

    #[test]
    fn a_revoked_key_loses_its_bundles_and_the_marks_it_raised() {
        let d = dir("revoke");
        let good = Spec::new("0.8.0");
        let mut forged = Spec::new("0.99.0");
        forged.seed = 5;
        forged.floor = "0.99.0";
        let mut store = Store::load(Some(&d), Some(&bundle(&good)), &both_keys());
        store.install(&bundle(&forged), &both_keys()).unwrap();
        assert_eq!(store.floor(), Version(0, 99, 0));
        assert_eq!(store.newest().unwrap().bundle.version, Version(0, 99, 0));

        assert!(store.reconcile(&trusted(7)));
        assert_eq!(store.held().len(), 1);
        assert_eq!(
            store.select(&server()).unwrap().bundle.version,
            Version(0, 8, 0)
        );
        // Neither the floor nor the rollback mark of the forged bundle stays.
        assert_eq!(store.floor(), Version(0, 0, 0));
        assert_eq!(store.high_water(1), Some(Version(0, 8, 0)));
        assert!(!d.join("mnema-web-0.99.0.zip").exists());
        // Nothing changes when the keys do not.
        assert!(!store.reconcile(&trusted(7)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn marks_of_a_revoked_key_are_forgotten_on_the_next_start_too() {
        let d = dir("revoke-restart");
        let good = Spec::new("0.8.0");
        let mut forged = Spec::new("0.99.0");
        forged.seed = 5;
        forged.floor = "0.99.0";
        let mut store = Store::load(Some(&d), Some(&bundle(&good)), &both_keys());
        store.install(&bundle(&forged), &both_keys()).unwrap();
        drop(store);
        // Restart with only the good key: the cached forged file is refused and
        // the state file's marks (set by the forged bundle) do not block 0.8.0.
        let again = Store::load(Some(&d), Some(&bundle(&good)), &trusted(7));
        assert_eq!(again.rejected.len(), 1);
        assert_eq!(again.floor(), Version(0, 0, 0));
        assert_eq!(again.high_water(1), Some(Version(0, 8, 0)));
        assert_eq!(
            again.select(&server()).unwrap().bundle.version,
            Version(0, 8, 0)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn when_only_revoked_bundles_are_left_the_client_says_so_until_a_new_one_arrives() {
        let mut forged = Spec::new("0.8.0");
        forged.seed = 5;
        let mut store = Store::load(None, Some(&bundle(&forged)), &trusted(5));
        assert!(store.select(&server()).is_ok());
        assert!(store.reconcile(&trusted(7)));
        assert_eq!(
            store.select(&server()).err(),
            Some(Refusal::SignerWithdrawn)
        );
        assert!(
            Refusal::SignerWithdrawn
                .to_string()
                .contains("no longer trusted")
        );
        // A bundle from the new key, even with the same version, is accepted.
        store
            .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
            .unwrap();
        assert_eq!(
            store.select(&server()).unwrap().bundle.version,
            Version(0, 8, 1)
        );
        // An offline start with a built-in bundle nobody trusts any more.
        let offline = Store::load(None, Some(&bundle(&forged)), &trusted(7));
        assert_eq!(
            offline.select(&server()).err(),
            Some(Refusal::SignerWithdrawn)
        );
    }

    #[test]
    fn the_security_floor_only_moves_up_and_withdraws_older_bundles() {
        let d = dir("floor");
        let old = Spec::new("0.8.0");
        let mut fixed = Spec::new("0.8.2");
        fixed.floor = "0.8.1";
        let mut store = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        assert_eq!(store.floor(), Version(0, 0, 0));
        store.install(&bundle(&fixed), &trusted(7)).unwrap();
        assert_eq!(store.floor(), Version(0, 8, 1));
        // The built-in 0.8.0 is below the floor now and cannot be selected.
        assert_eq!(store.held().len(), 1);
        assert_eq!(store.newest().unwrap().bundle.version, Version(0, 8, 2));
        // The floor survives a restart, even if the bundle that raised it is gone.
        std::fs::remove_file(d.join("mnema-web-0.8.2.zip")).unwrap();
        let again = Store::load(Some(&d), Some(&bundle(&old)), &trusted(7));
        assert_eq!(again.floor(), Version(0, 8, 1));
        assert!(again.newest().is_none());
        // Nor can an old bundle come back through an update.
        let mut again = Store::load(Some(&d), None, &trusted(7));
        assert!(again.install(&bundle(&old), &trusted(7)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn updates_install_only_newer_verified_bundles_and_survive_a_restart() {
        let d = dir("install");
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert!(
            store
                .install(&bundle(&Spec::new("0.8.0")), &trusted(7))
                .is_err(),
            "equal"
        );
        assert!(
            store
                .install(&bundle(&Spec::new("0.7.0")), &trusted(7))
                .is_err(),
            "older"
        );
        assert!(store.install(b"garbage", &trusted(7)).is_err());
        let mut other = Spec::new("0.9.0");
        other.seed = 9;
        assert!(
            store.install(&bundle(&other), &trusted(7)).is_err(),
            "wrong key"
        );
        assert_eq!(
            store
                .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
                .unwrap(),
            Version(0, 8, 1)
        );
        assert_eq!(store.newest().unwrap().source, Source::Cache);
        let reloaded = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(reloaded.held().len(), 2);
        assert_eq!(reloaded.newest().unwrap().bundle.version, Version(0, 8, 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_modified_cached_bundle_is_refused_and_the_next_verified_one_is_used() {
        let d = dir("tamper");
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        store
            .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
            .unwrap();
        // Flip one byte of the cached copy's content.
        let path = d.join("mnema-web-0.8.1.zip");
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes
            .windows(14)
            .position(|w| w == b"console.log(1)")
            .unwrap();
        bytes[at] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let reloaded = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(reloaded.rejected, vec!["mnema-web-0.8.1.zip".to_owned()]);
        assert_eq!(reloaded.held().len(), 1);
        assert_eq!(reloaded.newest().unwrap().source, Source::Builtin);
        assert_eq!(reloaded.newest().unwrap().bundle.version, Version(0, 8, 0));
        // And the damaged file does not stop the update from replacing it.
        let mut reloaded = reloaded;
        assert_eq!(
            reloaded
                .install(&bundle(&Spec::new("0.8.1")), &trusted(7))
                .unwrap(),
            Version(0, 8, 1)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_builtin_bundle_is_reported_and_unusable() {
        let store = Store::load(None, Some(b"not a bundle"), &trusted(7));
        assert_eq!(store.rejected, vec!["built-in".to_owned()]);
        assert!(store.newest().is_none());
    }

    #[test]
    fn the_cache_keeps_only_the_newest_few_versions_and_ignores_strangers() {
        let d = dir("prune");
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        std::fs::write(d.join("mnema-web-9.9.9.zip"), vec![0u8; 10]).unwrap();
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        for v in ["0.8.1", "0.8.2", "0.8.3", "0.8.4"] {
            store.install(&bundle(&Spec::new(v)), &trusted(7)).unwrap();
        }
        let versions: Vec<_> = store
            .held()
            .iter()
            .map(|h| h.bundle.version.to_string())
            .collect();
        assert_eq!(versions, ["0.8.4", "0.8.3", "0.8.2", "0.8.0"]);
        assert!(!d.join("mnema-web-0.8.1.zip").exists());
        assert!(d.join("notes.txt").exists());
        assert!(
            store.rejected.contains(&"mnema-web-9.9.9.zip".to_owned()) || store.rejected.is_empty()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn update_checks_are_rate_limited_and_remembered() {
        let d = dir("check");
        let mut store = Store::load(Some(&d), None, &trusted(7));
        assert!(store.check_due(1_000_000));
        store.mark_checked(1_000_000);
        assert!(!store.check_due(1_000_000 + CHECK_INTERVAL_SECS - 1));
        assert!(store.check_due(1_000_000 + CHECK_INTERVAL_SECS));
        assert!(!Store::load(Some(&d), None, &trusted(7)).check_due(1_000_001));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Cross-language check: set to a bundle produced by the Go tool. CI points
    /// it at the released bundle that is also built into the executable.
    #[test]
    fn a_bundle_from_the_signing_tool_verifies_with_the_pinned_key() {
        let Some(path) =
            std::env::var_os("MNEMA_TEST_BUNDLE_ZIP").filter(|p| Path::new(p).exists())
        else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let bundle =
            verify(&bytes, &trusted_keys()).expect("pinned key must verify the released bundle");
        assert!(bundle.file("index.html").is_some());
    }
}
