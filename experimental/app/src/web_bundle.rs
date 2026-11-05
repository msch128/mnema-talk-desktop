//! Signed web app bundles (mnema-talk `doc/desktop-trusted-client.md`).
//!
//! A bundle is one zip: `manifest.json`, `manifest.sig` (base64 Ed25519 over
//! the exact manifest bytes) and `app/<path>` for every file of the web app.
//! The host runs only bundles whose signature matches a key pinned in this
//! build. Nothing here trusts where bytes came from: the installer, the cache
//! and a download all go through [`verify`], and a cached bundle is verified
//! again on every start.
mod serve;
mod store;
mod verify;

pub use serve::{Held, Refusal, ServerInfo, Source};
pub use store::{CHECK_INTERVAL_SECS, Store, install_current};
pub use verify::Bundle;
#[cfg(test)]
pub use verify::{valid_path, verify};

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    path::{Path, PathBuf},
};

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

/// Writes through a synced `.partial` file and a rename, so a crash leaves
/// either the old or the new content and never a torn file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let mut file = std::fs::File::create(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&partial, path)
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::Cursor;
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
    use super::*;

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
}

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports this module to TypeScript; both read the same files and must give the
/// same answers (`linux/test/vectors.test.ts`).
#[cfg(test)]
mod shared_vectors {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        io::{Cursor, Read},
    };

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
        check_bundles(&load(include_str!("../../../test-vectors/bundle.json")));
    }

    /// The same manifest in forms a strict reader refuses: byte order mark, repeated
    /// keys, unknown fields, wrong types, data after the document.
    #[test]
    fn manifest_json() {
        check_bundles(&load(include_str!("../../../test-vectors/json.json")));
    }

    /// Hostile zip structures: both clients refuse each of them with the same message.
    #[test]
    fn zip_structures() {
        let v = load(include_str!("../../../test-vectors/zip.json"));
        check_bundles(&v);
        let open: Vec<&str> = v["bundles"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["result"]["ok"].is_object())
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            open,
            [
                "a plain stored bundle",
                "an entry that announces a data descriptor",
                "an entry with a regular file mode",
                "a link mode written by a system that does not use modes"
            ]
        );
    }

    fn check_bundles(v: &Value) {
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
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "over the limit when the limits were introduced; split it"
    )]
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
