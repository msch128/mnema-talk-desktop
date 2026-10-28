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

/// Whether `id` has the shape of a key or instance id: the first 8 bytes of a
/// SHA-256 as 16 lowercase hex characters (see [`key_id`], `instances::id_for`).
pub(crate) fn is_hex_id16(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
        .ok_or(NOT_TRUSTED)?;

    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| "unreadable manifest")?;
    let Compat {
        version,
        min_server_version,
        security_floor,
    } = check_manifest(&manifest, &signer.id)?;
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
        min_server_version,
        security_floor,
        key_id: manifest.key_id,
        commit: manifest.commit,
        files,
    })
}

/// The version numbers of a checked manifest.
struct Compat {
    version: Version,
    min_server_version: Version,
    security_floor: Version,
}

fn check_manifest(m: &Manifest, signer: &str) -> Result<Compat, String> {
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
    Ok(Compat {
        version,
        min_server_version: min,
        security_floor: floor,
    })
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
    /// Earlier states: marks without a signer. Read as the unknown signer ""
    /// (always counted), never written again.
    security_floor: Option<Version>,
    #[serde(default)]
    high_water: BTreeMap<u32, Version>,
    /// Highest `security_floor` each signing key ever declared (key id).
    #[serde(default)]
    floors: BTreeMap<String, Version>,
    /// Highest bundle version ever verified per `web_client_api` and signing
    /// key. The mark in force is the highest one of a key that is still
    /// trusted: bundles below it are never selected, whatever the server says.
    #[serde(default)]
    marks: BTreeMap<u32, BTreeMap<String, Version>>,
    last_check: u64,
}

/// Writes through a temporary file and a rename, so a crash leaves either the
/// old or the new content and never a torn file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

const STATE_FILE: &str = "state.json";
const STATE_BACKUP: &str = "state.json.bak";
/// Why a bundle did not verify when its signer is simply not trusted (now).
/// Such a file stays on disk: the key may be trusted again.
const NOT_TRUSTED: &str = "signature does not match a trusted key";

/// The recorded marks. A state file that is there but cannot be parsed falls
/// back to the last good copy, so damage does not drop the floors and marks.
/// A missing file is a fresh start: the marks are rebuilt from the bundles.
fn read_state(dir: &Path) -> State {
    let parse = |name: &str| std::fs::read(dir.join(name)).ok();
    match parse(STATE_FILE) {
        None => State::default(),
        Some(raw) => serde_json::from_slice(&raw)
            .ok()
            .or_else(|| parse(STATE_BACKUP).and_then(|raw| serde_json::from_slice(&raw).ok()))
            .unwrap_or_default(),
    }
}

/// Marks from before signers were recorded.
const UNKNOWN_SIGNER: &str = "";

/// Verified bundles in memory plus the cache on disk.
pub struct Store {
    dir: Option<PathBuf>,
    /// The floor and marks in force (see `recompute`).
    floor: Version,
    high_water: BTreeMap<u32, Version>,
    floors: BTreeMap<String, Version>,
    marks: BTreeMap<u32, BTreeMap<String, Version>>,
    /// Ids of the keys trusted right now.
    trusted: Vec<String>,
    last_check: u64,
    /// Bundles were dropped or refused because their key is not trusted (and
    /// no trusted bundle was installed since). Only picks the message when
    /// nothing can be served; other rejects do not count.
    withdrawn: bool,
    held: Vec<Held>,
    /// Verified bundles below the floor. Never served; they return if the key
    /// that raised the floor is revoked.
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
        let state = dir.map(read_state).unwrap_or_default();
        let mut verified: Vec<Held> = Vec::new();
        let mut rejected = Vec::new();
        let mut withdrawn = false;
        if let Some(bytes) = builtin {
            match verify(bytes, keys) {
                Ok(bundle) => verified.push(Held {
                    bundle: Arc::new(bundle),
                    source: Source::Builtin,
                }),
                Err(reason) => {
                    withdrawn |= reason == NOT_TRUSTED;
                    rejected.push("built-in".to_owned())
                }
            }
        }
        let mut usable = 0;
        for (named, path) in cached_files(dir) {
            if usable >= MAX_CACHED_FILES {
                break;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let loaded = std::fs::metadata(&path)
                .ok()
                .filter(|m| m.len() <= MAX_ZIP)
                .and_then(|_| std::fs::read(&path).ok())
                .ok_or_else(|| "unreadable".to_owned())
                .and_then(|bytes| verify(&bytes, keys))
                .and_then(|bundle| {
                    if bundle.version == named {
                        Ok(bundle)
                    } else {
                        Err("file name does not match the bundle".to_owned())
                    }
                });
            match loaded {
                Ok(bundle) if !verified.iter().any(|h| h.bundle.version == bundle.version) => {
                    usable += 1;
                    verified.push(Held {
                        bundle: Arc::new(bundle),
                        source: Source::Cache,
                    })
                }
                // Same version as one already held: the file is redundant.
                Ok(_) => {
                    let _ = std::fs::remove_file(&path);
                }
                Err(reason) => {
                    // A file nobody can use is removed; one whose signer is
                    // not trusted right now stays, the key may return.
                    if reason == NOT_TRUSTED {
                        withdrawn = true;
                    } else {
                        let _ = std::fs::remove_file(&path);
                    }
                    rejected.push(name);
                }
            }
        }
        let mut floors = state.floors;
        if let Some(floor) = state.security_floor {
            let entry = floors.entry(UNKNOWN_SIGNER.to_owned()).or_insert(floor);
            *entry = (*entry).max(floor);
        }
        let mut marks = state.marks;
        for (api, version) in state.high_water {
            let entry = marks
                .entry(api)
                .or_default()
                .entry(UNKNOWN_SIGNER.to_owned())
                .or_insert(version);
            *entry = (*entry).max(version);
        }
        let mut store = Store {
            dir: dir.map(Path::to_path_buf),
            floor: Version(0, 0, 0),
            high_water: BTreeMap::new(),
            floors,
            marks,
            trusted: keys.iter().map(|k| k.id.clone()).collect(),
            last_check: state.last_check,
            withdrawn,
            held: verified,
            shelved: Vec::new(),
            rejected,
        };
        store.raise_from_held();
        store.recompute();
        store.apply_floor();
        store.persist();
        store
    }

    fn persist(&self) {
        let Some(dir) = &self.dir else { return };
        let state = State {
            security_floor: None,
            high_water: BTreeMap::new(),
            floors: self.floors.clone(),
            marks: self.marks.clone(),
            last_check: self.last_check,
        };
        let Ok(raw) = serde_json::to_vec(&state) else {
            return;
        };
        // The file in place is the last good one: keep it as the fallback.
        let current = dir.join(STATE_FILE);
        if std::fs::read(&current).is_ok_and(|old| serde_json::from_slice::<State>(&old).is_ok()) {
            let _ = std::fs::copy(&current, dir.join(STATE_BACKUP));
        }
        if let Err(error) = write_atomic(&current, &raw) {
            eprintln!("Mnema Desktop: cannot save the bundle state: {error}");
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
            // Bundles for this contract are held but all below the newest this
            // client has accepted. If the newest one was dropped because its
            // key was revoked, say that instead.
            None if self
                .held
                .iter()
                .any(|h| h.bundle.web_client_api == server.web_client_api) =>
            {
                Err(if self.withdrawn {
                    Refusal::SignerWithdrawn
                } else {
                    Refusal::RolledBack
                })
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
        // Per contract: a patch for an older `web_client_api` is not shadowed
        // by a newer bundle of another one. A version is held only once (it
        // names the cached file).
        if self.held.iter().any(|h| {
            h.bundle.version == version
                || (h.bundle.web_client_api == bundle.web_client_api && h.bundle.version > version)
        }) {
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
        self.trusted = keys.iter().map(|k| k.id.clone()).collect();
        self.withdrawn = false;
        self.raise_marks(&bundle);
        self.recompute();
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
    /// other key are dropped (cached files removed), and the floor and marks
    /// are recomputed from the keys still trusted. Returns whether anything
    /// was dropped.
    pub fn reconcile(&mut self, keys: &[TrustedKey]) -> bool {
        self.trusted = keys.iter().map(|k| k.id.clone()).collect();
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
        if !dropped.is_empty() {
            self.withdrawn = true;
        }
        // The marks in force now come from still-trusted keys only.
        self.recompute();
        self.apply_floor();
        self.persist();
        !dropped.is_empty()
    }

    /// Applies the caps of a newly taken trust statement: the recorded marks
    /// per web client API and the floors are lowered to at most the cap, and
    /// held bundles above a cap are dropped. This clears a mark raised by a
    /// forged bundle even when no trusted bundle exists yet.
    pub fn apply_caps(&mut self, caps: &BTreeMap<u32, Version>) {
        let Some(highest) = caps.values().max().copied() else {
            return;
        };
        for (api, cap) in caps {
            for mark in self.marks.entry(*api).or_default().values_mut() {
                *mark = (*mark).min(*cap);
            }
        }
        for floor in self.floors.values_mut() {
            *floor = (*floor).min(highest);
        }
        let over = |h: &Held| {
            caps.get(&h.bundle.web_client_api)
                .is_some_and(|c| h.bundle.version > *c)
        };
        // Their cached files go too: left on disk, the next start would verify
        // them again and raise the marks anew.
        if let Some(dir) = &self.dir {
            for h in self.held.iter().chain(&self.shelved) {
                if over(h) && h.source == Source::Cache {
                    let _ = std::fs::remove_file(
                        dir.join(format!("mnema-web-{}.zip", h.bundle.version)),
                    );
                }
            }
        }
        self.held.retain(|h| !over(h));
        self.shelved.retain(|h| !over(h));
        self.recompute();
        self.apply_floor();
        self.persist();
    }

    /// Floor and high-water marks in force: the highest recorded by any key
    /// that is trusted now (or whose signer is unknown). Revoking a key clears
    /// what it raised and nothing a trusted key reached or declared.
    fn recompute(&mut self) {
        let counts = |key: &String| key == UNKNOWN_SIGNER || self.trusted.contains(key);
        self.floor = self
            .floors
            .iter()
            .filter(|(k, _)| counts(k))
            .map(|(_, v)| *v)
            .max()
            .unwrap_or(Version(0, 0, 0));
        self.high_water = self
            .marks
            .iter()
            .filter_map(|(api, by_key)| {
                by_key
                    .iter()
                    .filter(|(k, _)| counts(k))
                    .map(|(_, v)| *v)
                    .max()
                    .map(|v| (*api, v))
            })
            .collect();
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

    /// Serves bundles at or above the floor, newest first.
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
        let floor = self
            .floors
            .entry(bundle.key_id.clone())
            .or_insert(bundle.security_floor);
        *floor = (*floor).max(bundle.security_floor);
        let mark = self
            .marks
            .entry(bundle.web_client_api)
            .or_default()
            .entry(bundle.key_id.clone())
            .or_insert(bundle.version);
        *mark = (*mark).max(bundle.version);
    }

    /// Keeps the built-in bundle and, per web client API, the newest few
    /// cached ones, served or shelved; the rest is dropped with its file. A
    /// shelved bundle only matters if the key that raised the floor is
    /// revoked, and then the newest of them are the ones that return.
    fn prune(&mut self) {
        let mut dropped = Vec::new();
        for list in [&mut self.held, &mut self.shelved] {
            let mut cached: BTreeMap<u32, usize> = BTreeMap::new();
            list.retain(|h| {
                if h.source == Source::Builtin {
                    return true;
                }
                let n = cached.entry(h.bundle.web_client_api).or_default();
                *n += 1;
                if *n > KEEP_CACHED {
                    dropped.push(h.bundle.version);
                    false
                } else {
                    true
                }
            });
        }
        if let Some(dir) = &self.dir {
            for version in dropped {
                let _ = std::fs::remove_file(dir.join(format!("mnema-web-{version}.zip")));
            }
        }
    }
}

/// Installs a bundle against the keys in force *now*. The store lock is taken
/// first and the key set is read while holding it; trust updates take the same
/// locks in the same order, so a bundle cannot be installed with keys that a
/// concurrent statement has just withdrawn.
pub fn install_current(
    store: &std::sync::Mutex<Store>,
    keys: &std::sync::Mutex<Vec<TrustedKey>>,
    bytes: &[u8],
) -> Result<Version, String> {
    let mut store = store.lock().map_err(|_| "busy")?;
    let keys = keys.lock().map_err(|_| "busy")?;
    store.install(bytes, &keys)
}

/// The cached bundle files by the version in their name, newest first. A
/// `.partial` left by an interrupted install is removed.
fn cached_files(dir: Option<&Path>) -> Vec<(Version, PathBuf)> {
    let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("mnema-web-") else {
            continue;
        };
        if !entry.path().is_file() {
            continue;
        }
        if rest.ends_with(".zip.partial") {
            let _ = std::fs::remove_file(entry.path());
        } else if let Some(version) = rest.strip_suffix(".zip").and_then(Version::parse) {
            files.push((version, entry.path()));
        }
    }
    files.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
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
        for ok in ["index.html", "assets/a-b.js", "\u{fc}n\u{ef}/x.js"] {
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

    fn forged_and_good(d: &Path) -> Store {
        let good = Spec::new("0.8.0");
        let mut forged = Spec::new("0.99.0");
        forged.seed = 5;
        forged.floor = "0.99.0";
        let mut store = Store::load(Some(d), Some(&bundle(&good)), &both_keys());
        store.install(&bundle(&forged), &both_keys()).unwrap();
        store
    }

    #[test]
    fn a_forged_mark_of_a_revoked_key_is_cleared() {
        let d = dir("revoke");
        let mut store = forged_and_good(&d);
        assert_eq!(store.floor(), Version(0, 99, 0));
        assert!(store.reconcile(&trusted(7)));
        assert!(!d.join("mnema-web-0.99.0.zip").exists());
        assert_eq!(store.floor(), Version(0, 0, 0));
        assert_eq!(store.high_water(1), Some(Version(0, 8, 0)));
        // The good built-in bundle, held back by the forged floor, runs again.
        assert_eq!(
            store.select(&server()).unwrap().bundle.version,
            Version(0, 8, 0)
        );
        assert!(!store.reconcile(&trusted(7)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_cleared_marks_stay_cleared_after_a_restart() {
        let d = dir("revoke-restart");
        drop(forged_and_good(&d));
        let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(again.rejected.len(), 1);
        assert_eq!(again.floor(), Version(0, 0, 0));
        assert_eq!(again.high_water(1), Some(Version(0, 8, 0)));
        assert_eq!(
            again.select(&server()).unwrap().bundle.version,
            Version(0, 8, 0)
        );
        // With both keys trusted the forged marks are in force again.
        let both = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &both_keys());
        assert_eq!(both.floor(), Version(0, 99, 0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn marks_and_floors_of_still_trusted_keys_are_kept() {
        let d = dir("keep");
        let mut legit = Spec::new("0.9.0");
        legit.floor = "0.8.5";
        let mut forged = Spec::new("0.99.0");
        forged.seed = 5;
        forged.floor = "0.99.0";
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &both_keys());
        store.install(&bundle(&legit), &both_keys()).unwrap();
        store.install(&bundle(&forged), &both_keys()).unwrap();
        assert!(store.reconcile(&trusted(7)));
        // Never below what the trusted key reached or declared.
        assert_eq!(store.floor(), Version(0, 8, 5));
        assert_eq!(store.high_water(1), Some(Version(0, 9, 0)));
        assert_eq!(
            store.select(&server()).unwrap().bundle.version,
            Version(0, 9, 0)
        );
        // The built-in 0.8.0 stays below the trusted floor.
        assert_eq!(store.held().len(), 1);
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
    fn a_cap_from_the_statement_lowers_marks_even_of_unknown_or_trusted_signers() {
        // A state written before signers were recorded: forged marks of an
        // unknown signer always count, and only a cap can clear them.
        let d = dir("cap");
        std::fs::write(
            d.join("state.json"),
            serde_json::json!({
                "security_floor": [0, 99, 0],
                "high_water": {"1": [0, 99, 0]},
                "last_check": 0
            })
            .to_string(),
        )
        .unwrap();
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(store.floor(), Version(0, 99, 0));
        assert!(store.select(&server()).is_err());
        let caps = BTreeMap::from([(1, Version(0, 8, 0))]);
        store.apply_caps(&caps);
        assert_eq!(store.floor(), Version(0, 8, 0));
        assert_eq!(store.high_water(1), Some(Version(0, 8, 0)));
        assert_eq!(
            store.select(&server()).unwrap().bundle.version,
            Version(0, 8, 0)
        );
        // The cap survives a restart in the recorded marks.
        let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(again.floor(), Version(0, 8, 0));
        // Bundles above the cap are dropped, even from a trusted key; other
        // contracts keep their own marks; no caps change nothing.
        let mut high = Spec::new("0.99.0");
        high.floor = "0.5.0";
        let mut api2 = Spec::new("1.0.0");
        api2.api = 2;
        let mut store = Store::load(None, Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        store.install(&bundle(&high), &trusted(7)).unwrap();
        store.install(&bundle(&api2), &trusted(7)).unwrap();
        store.apply_caps(&BTreeMap::new());
        assert_eq!(store.high_water(1), Some(Version(0, 99, 0)));
        store.apply_caps(&BTreeMap::from([(1, Version(0, 9, 0))]));
        assert_eq!(store.high_water(1), Some(Version(0, 9, 0)));
        assert_eq!(store.high_water(2), Some(Version(1, 0, 0)));
        assert!(
            store
                .held()
                .iter()
                .filter(|h| h.bundle.web_client_api == 1)
                .all(|h| h.bundle.version <= Version(0, 9, 0))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn installs_use_the_keys_in_force_when_the_store_lock_is_held() {
        use std::sync::Mutex;
        let store = Mutex::new(Store::load(None, None, &both_keys()));
        let keys = Mutex::new(both_keys());
        let mut forged = Spec::new("0.8.0");
        forged.seed = 5;
        // A statement withdrawing key 5 lands after the keys were first read.
        *keys.lock().unwrap() = trusted(7);
        assert!(install_current(&store, &keys, &bundle(&forged)).is_err());
        assert!(store.lock().unwrap().held().is_empty());
        assert!(install_current(&store, &keys, &bundle(&Spec::new("0.8.0"))).is_ok());
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
        assert_eq!(store.rejected, vec!["mnema-web-9.9.9.zip".to_owned()]);
        assert!(!d.join("mnema-web-9.9.9.zip").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_patch_for_an_older_contract_is_not_shadowed_by_a_newer_one() {
        let mut api2 = Spec::new("0.9.0");
        api2.api = 2;
        let mut store = Store::load(None, Some(&bundle(&api2)), &trusted(7));
        // 0.8.5 speaks contract 1: it is new for that contract.
        assert_eq!(
            store
                .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
                .unwrap(),
            Version(0, 8, 5)
        );
        // Same contract and not newer, or a version already held: refused.
        assert!(
            store
                .install(&bundle(&Spec::new("0.8.4")), &trusted(7))
                .is_err()
        );
        assert!(
            store
                .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
                .is_err()
        );
        let info = |api| ServerInfo {
            version: None,
            web_client_api: api,
        };
        assert_eq!(
            store.select(&info(1)).unwrap().bundle.version,
            Version(0, 8, 5)
        );
        assert_eq!(
            store.select(&info(2)).unwrap().bundle.version,
            Version(0, 9, 0)
        );
    }

    #[test]
    fn only_a_withdrawn_signer_picks_the_withdrawn_message() {
        // A damaged file is not a withdrawn signer, and it is removed.
        let d = dir("withdrawn-damaged");
        std::fs::write(d.join("mnema-web-0.1.0.zip"), b"garbage").unwrap();
        let store = Store::load(Some(&d), None, &trusted(7));
        assert_eq!(store.rejected, vec!["mnema-web-0.1.0.zip".to_owned()]);
        assert_eq!(store.empty_reason(), Refusal::NoBundle);
        assert!(!d.join("mnema-web-0.1.0.zip").exists());
        let _ = std::fs::remove_dir_all(&d);

        // A bundle of a key that is not trusted is, and its file is kept.
        let d = dir("withdrawn-key");
        let mut other = Spec::new("0.2.0");
        other.seed = 5;
        std::fs::write(d.join("mnema-web-0.2.0.zip"), bundle(&other)).unwrap();
        let mut store = Store::load(Some(&d), None, &trusted(7));
        assert_eq!(store.empty_reason(), Refusal::SignerWithdrawn);
        assert!(d.join("mnema-web-0.2.0.zip").exists());
        // A trusted bundle installed afterwards clears it.
        store
            .install(&bundle(&Spec::new("0.3.0")), &trusted(7))
            .unwrap();
        assert!(!store.withdrawn);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_state_file_falls_back_to_the_last_good_one() {
        let d = dir("state-backup");
        let mut floor = Spec::new("0.8.4");
        floor.floor = "0.8.2";
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        store.install(&bundle(&floor), &trusted(7)).unwrap();
        store
            .install(&bundle(&Spec::new("0.8.5")), &trusted(7))
            .unwrap();
        assert!(!d.join("state.tmp").exists());
        // A torn write, and the bundles that carried the floor are gone.
        std::fs::write(d.join("state.json"), b"{\"floors\":").unwrap();
        for v in ["0.8.4", "0.8.5"] {
            std::fs::remove_file(d.join(format!("mnema-web-{v}.zip"))).unwrap();
        }
        let again = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        assert_eq!(again.floor(), Version(0, 8, 2));
        // The next save is whole again.
        let raw = std::fs::read(d.join("state.json")).unwrap();
        assert!(serde_json::from_slice::<State>(&raw).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cached_files_are_ordered_by_version_and_leftovers_are_removed() {
        let d = dir("scan");
        for name in [
            "mnema-web-0.9.0.zip",
            "mnema-web-0.10.0.zip",
            "mnema-web-1.2.0.zip",
            "mnema-web-1.2.0.zip.partial",
            "mnema-web-x.zip",
            "other.zip",
        ] {
            std::fs::write(d.join(name), b"x").unwrap();
        }
        let versions: Vec<_> = cached_files(Some(&d)).into_iter().map(|(v, _)| v).collect();
        assert_eq!(
            versions,
            [Version(1, 2, 0), Version(0, 10, 0), Version(0, 9, 0)]
        );
        assert!(!d.join("mnema-web-1.2.0.zip.partial").exists());
        assert!(d.join("mnema-web-x.zip").exists());
        assert!(d.join("other.zip").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_duplicate_of_the_builtin_version_is_removed_from_the_cache() {
        let d = dir("duplicate");
        let spec = Spec::new("0.8.0");
        std::fs::write(d.join("mnema-web-0.8.0.zip"), bundle(&spec)).unwrap();
        let store = Store::load(Some(&d), Some(&bundle(&spec)), &trusted(7));
        assert_eq!(store.held().len(), 1);
        assert!(!d.join("mnema-web-0.8.0.zip").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn shelved_bundles_are_bounded_too() {
        let d = dir("shelved");
        let mut store = Store::load(Some(&d), Some(&bundle(&Spec::new("0.8.0"))), &trusted(7));
        for v in ["0.8.1", "0.8.2", "0.8.3"] {
            store.install(&bundle(&Spec::new(v)), &trusted(7)).unwrap();
        }
        for v in ["0.9.0", "0.9.1"] {
            let mut raised = Spec::new(v);
            raised.floor = v;
            store.install(&bundle(&raised), &trusted(7)).unwrap();
        }
        assert_eq!(store.newest().unwrap().bundle.version, Version(0, 9, 1));
        assert!(!d.join("mnema-web-0.8.1.zip").exists());
        for v in ["0.8.2", "0.8.3", "0.9.0", "0.9.1"] {
            assert!(d.join(format!("mnema-web-{v}.zip")).exists(), "{v}");
        }
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
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports this module to TypeScript; both read the same files and must give the
/// same answers (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::*;
    use serde_json::Value;

    fn load(text: &str) -> Value {
        serde_json::from_str(text).expect("the vector file is JSON")
    }

    fn strings(value: &Value) -> Vec<&str> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect()
    }

    fn keys(hexes: &[&str]) -> Vec<TrustedKey> {
        hexes.iter().map(|h| parse_key(h).unwrap()).collect()
    }

    fn version(value: &Value) -> Option<Version> {
        value.as_array().map(|a| {
            Version(
                a[0].as_u64().unwrap() as u32,
                a[1].as_u64().unwrap() as u32,
                a[2].as_u64().unwrap() as u32,
            )
        })
    }

    #[test]
    fn versions_and_paths() {
        let v = load(include_str!("../../../test-vectors/bundle.json"));
        for c in v["versions"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            assert_eq!(Version::parse(input), version(&c["parsed"]), "{input:?}");
        }
        for c in v["paths"].as_array().unwrap() {
            let path = c["path"].as_str().unwrap();
            assert_eq!(valid_path(path), c["valid"].as_bool().unwrap(), "{path:?}");
        }
    }

    #[test]
    fn bundles() {
        let v = load(include_str!("../../../test-vectors/bundle.json"));
        for c in v["bundles"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let zip = STANDARD.decode(c["zip"].as_str().unwrap()).unwrap();
            let result = verify(&zip, &keys(&strings(&c["trusted"])));
            match (&result, &c["result"]["ok"], &c["result"]["error"]) {
                (Ok(bundle), ok, Value::Null) => {
                    assert_eq!(
                        bundle.version.to_string(),
                        ok["version"].as_str().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        bundle.web_client_api as u64,
                        ok["web_client_api"].as_u64().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        bundle.min_server_version.to_string(),
                        ok["min_server_version"].as_str().unwrap(),
                        "{name}"
                    );
                    assert_eq!(
                        bundle.security_floor.to_string(),
                        ok["security_floor"].as_str().unwrap(),
                        "{name}"
                    );
                    assert_eq!(bundle.key_id, ok["key_id"].as_str().unwrap(), "{name}");
                    assert_eq!(bundle.commit, ok["commit"].as_str().unwrap(), "{name}");
                    for (path, hash) in ok["files"].as_object().unwrap() {
                        let body = bundle
                            .file(path)
                            .unwrap_or_else(|| panic!("{name}: {path}"));
                        assert_eq!(
                            &hex::encode(Sha256::digest(body)),
                            hash.as_str().unwrap(),
                            "{name}: {path}"
                        );
                    }
                }
                (Err(message), Value::Null, error) => {
                    assert_eq!(message, error.as_str().unwrap(), "{name}");
                }
                _ => panic!(
                    "{name}: the answers differ: {:?} against {}",
                    result.as_ref().map(|b| b.version),
                    c["result"]
                ),
            }
        }
    }

    fn named_keys(v: &Value, names: &Value) -> Vec<TrustedKey> {
        strings(names)
            .into_iter()
            .map(|n| parse_key(v["keys"][n].as_str().unwrap()).unwrap())
            .collect()
    }

    fn refusal_kind(refusal: &Refusal) -> &'static str {
        match refusal {
            Refusal::NoBundle => "no_bundle",
            Refusal::ServerNewer => "server_newer",
            Refusal::ServerTooOld { .. } => "server_too_old",
            Refusal::ServerContractRetired => "server_contract_retired",
            Refusal::RolledBack => "rolled_back",
            Refusal::SignerWithdrawn => "signer_withdrawn",
        }
    }

    #[test]
    fn stores() {
        let v = load(include_str!("../../../test-vectors/store.json"));
        let bytes = |name: &Value| {
            STANDARD
                .decode(v["bundles"][name.as_str().unwrap()].as_str().unwrap())
                .unwrap()
        };
        for (index, s) in v["scenarios"].as_array().unwrap().iter().enumerate() {
            let name = s["name"].as_str().unwrap();
            let dir = s["persist"].as_bool().unwrap().then(|| {
                let d =
                    std::env::temp_dir().join(format!("mnema-vec-{}-{index}", std::process::id()));
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).unwrap();
                d
            });
            let builtin = s["builtin"]
                .as_str()
                .map(|b| bytes(&Value::String(b.to_owned())));
            let mut store = Store::load(
                dir.as_deref(),
                builtin.as_deref(),
                &named_keys(&v, &s["trusted"]),
            );
            for (n, step) in s["steps"].as_array().unwrap().iter().enumerate() {
                let op = step["op"].as_str().unwrap();
                let at = format!("{name} #{n} {op}");
                match op {
                    "install" => {
                        let got = store
                            .install(&bytes(&step["bundle"]), &named_keys(&v, &step["trusted"]));
                        match (&got, &step["expect"]["ok"], &step["expect"]["error"]) {
                            (Ok(version), ok, Value::Null) => {
                                assert_eq!(version.to_string(), ok.as_str().unwrap(), "{at}")
                            }
                            (Err(message), Value::Null, error) => {
                                assert_eq!(message, error.as_str().unwrap(), "{at}")
                            }
                            _ => panic!("{at}: {got:?} against {}", step["expect"]),
                        }
                    }
                    "select" => {
                        let server = ServerInfo {
                            version: step["server"]["version"]
                                .as_str()
                                .map(|s| Version::parse(s).unwrap()),
                            web_client_api: step["server"]["api"].as_u64().unwrap() as u32,
                        };
                        match store.select(&server) {
                            Ok(held) => assert_eq!(
                                held.bundle.version.to_string(),
                                step["expect"]["ok"].as_str().unwrap(),
                                "{at}"
                            ),
                            Err(refusal) => {
                                assert_eq!(
                                    refusal_kind(&refusal),
                                    step["expect"]["refusal"].as_str().unwrap(),
                                    "{at}"
                                );
                                if let Refusal::ServerTooOld { needs } = refusal {
                                    assert_eq!(
                                        needs.to_string(),
                                        step["expect"]["needs"].as_str().unwrap(),
                                        "{at}"
                                    );
                                }
                            }
                        }
                    }
                    "reconcile" => {
                        assert_eq!(
                            store.reconcile(&named_keys(&v, &step["trusted"])),
                            step["expect"].as_bool().unwrap(),
                            "{at}"
                        );
                    }
                    "caps" => {
                        let caps: BTreeMap<u32, Version> = step["caps"]
                            .as_object()
                            .unwrap()
                            .iter()
                            .map(|(k, c)| {
                                (
                                    k.parse().unwrap(),
                                    Version::parse(c.as_str().unwrap()).unwrap(),
                                )
                            })
                            .collect();
                        store.apply_caps(&caps);
                    }
                    "state" => {
                        let expect = &step["expect"];
                        assert_eq!(
                            store.floor().to_string(),
                            expect["floor"].as_str().unwrap(),
                            "{at}"
                        );
                        let mut high_water = serde_json::Map::new();
                        for api in 1..=3u32 {
                            if let Some(mark) = store.high_water(api) {
                                high_water.insert(api.to_string(), Value::String(mark.to_string()));
                            }
                        }
                        assert_eq!(Value::Object(high_water), expect["high_water"], "{at}");
                        let held: Vec<String> = store
                            .held()
                            .iter()
                            .map(|h| format!("{}:{}", h.bundle.version, h.source.as_str()))
                            .collect();
                        assert_eq!(held, strings(&expect["held"]), "{at}");
                        assert_eq!(
                            store.rejected.len() as u64,
                            expect["rejected"].as_u64().unwrap(),
                            "{at}"
                        );
                    }
                    "restart" => {
                        store = Store::load(
                            dir.as_deref(),
                            builtin.as_deref(),
                            &named_keys(&v, &step["trusted"]),
                        );
                    }
                    "remove_cached" => {
                        let file = format!("mnema-web-{}.zip", step["version"].as_str().unwrap());
                        std::fs::remove_file(dir.as_ref().unwrap().join(file)).unwrap();
                    }
                    "forget_state" => {
                        let _ = std::fs::remove_file(dir.as_ref().unwrap().join("state.json"));
                    }
                    other => panic!("{at}: unknown step {other}"),
                }
            }
            if let Some(d) = dir {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }

    /// The signed release bundle CI stages (`MNEMA_TEST_BUNDLE_ZIP`, from the
    /// mnema-talk GitHub release) against the key pinned in this build, so a
    /// change to the signature or hash crates is checked on real data, not only
    /// on bundles the tests sign themselves; every tampered form is refused.
    #[test]
    fn a_real_signed_release_bundle_verifies_and_every_tampered_form_is_refused() {
        let Some(path) =
            std::env::var_os("MNEMA_TEST_BUNDLE_ZIP").filter(|p| Path::new(p).exists())
        else {
            return;
        };
        let data = std::fs::read(path).unwrap();
        let keys = trusted_keys();
        let bundle = verify(&data, &keys).expect("the release bundle verifies");
        assert!(bundle.file("index.html").is_some());

        type Entries = Vec<(String, Vec<u8>)>;
        let mut entries: Entries = {
            let mut zip = zip::ZipArchive::new(Cursor::new(&data[..])).unwrap();
            (0..zip.len())
                .filter_map(|i| {
                    let mut f = zip.by_index(i).unwrap();
                    if f.is_dir() {
                        return None;
                    }
                    let mut body = Vec::new();
                    f.read_to_end(&mut body).unwrap();
                    Some((f.name().unwrap().into_owned(), body))
                })
                .collect()
        };
        // Repacked unchanged: still valid, so the refusals below are the edits.
        verify(&fixture::zip_of(&entries), &keys).expect("the repacked bundle verifies");
        let refused = |edit: &dyn Fn(&mut Entries)| {
            let mut copy = entries.clone();
            edit(&mut copy);
            verify(&fixture::zip_of(&copy), &keys).is_err()
        };
        let flip = |e: &mut Entries, name: &str| {
            let body = &mut e.iter_mut().find(|(n, _)| n == name).unwrap().1;
            body[0] ^= 1;
        };
        assert!(refused(&|e| flip(e, "app/index.html")));
        assert!(refused(&|e| flip(e, "manifest.json")));
        assert!(refused(&|e| {
            let sig = &mut e.iter_mut().find(|(n, _)| n == "manifest.sig").unwrap().1;
            // A different, well-formed signature of the right length.
            let mut other = STANDARD.decode(sig.trim_ascii()).unwrap();
            other[0] ^= 1;
            *sig = STANDARD.encode(other).into_bytes();
        }));
        assert!(refused(&|e| e.push(("app/evil.js".into(), b"x".to_vec()))));
        assert!(refused(&|e| e.retain(|(n, _)| n != "manifest.sig")));
        // The right bundle under a key that did not sign it.
        assert!(verify(&data, &fixture::trusted(9)).is_err());
        entries.clear();
    }
}
