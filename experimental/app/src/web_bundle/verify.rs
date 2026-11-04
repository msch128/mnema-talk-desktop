//! Checking one bundle: the zip rules, the signature, the manifest and the
//! hash of every file.
use super::{TrustedKey, Version};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::Signature;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read},
};

const MAX_ENTRIES: usize = 4000;
const MAX_FILE: u64 = 64 << 20;
const MAX_TOTAL: u64 = 256 << 20;
const MAX_MANIFEST: u64 = 4 << 20;
const MAX_SIGNATURE: u64 = 1024;
const MAX_PATH: usize = 512;

/// Why a bundle did not verify when its signer is simply not trusted (now).
/// Such a file stays on disk: the key may be trusted again.
pub(super) const NOT_TRUSTED: &str = "signature does not match a trusted key";

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
    #[serde(deserialize_with = "crate::web_json::objects")]
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
/// slashes, no empty or dot-leading segment (hidden entries, `.` and `..`), no
/// control character.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path
            .chars()
            .any(|c| c < '\u{20}' || c == '\u{7f}' || c == '\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && !segment.starts_with('.'))
}

/// The archive rules the zip crate does not enforce, the same as the Linux
/// client's `zip.ts`: the end record is the last 22 bytes (no comment, nothing
/// appended, no ZIP64 locator), the first entry starts the file, the central
/// directory ends where the end record begins, and no size or offset is a ZIP64
/// marker, and no name is used twice. Returns, per entry in directory order, whether it must not be read:
/// encrypted, or a link, directory or device written by a system with Unix modes.
fn check_layout(data: &[u8]) -> Result<Vec<bool>, String> {
    const BAD: &str = "not a readable zip";
    let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    if data.len() < 22 {
        return Err(BAD.into());
    }
    let eocd = data.len() - 22;
    if u32_at(eocd) != 0x0605_4b50 || u16_at(eocd + 20) != 0 {
        return Err(BAD.into());
    }
    if data.len() >= 42 && u32_at(eocd - 20) == 0x0706_4b50 {
        return Err(BAD.into());
    }
    if u32_at(0) != 0x0403_4b50 {
        return Err(BAD.into());
    }
    let (disk, cd_disk, on_disk, total) = (
        u16_at(eocd + 4),
        u16_at(eocd + 6),
        u16_at(eocd + 8),
        u16_at(eocd + 10),
    );
    let (cd_size, cd_offset) = (u32_at(eocd + 12) as usize, u32_at(eocd + 16) as usize);
    if disk != 0
        || cd_disk != 0
        || on_disk != total
        || total == 0xffff
        || cd_size == 0xffff_ffff
        || cd_offset == 0xffff_ffff
        || cd_offset + cd_size != eocd
    {
        return Err(BAD.into());
    }
    let mut refused = Vec::with_capacity(total as usize);
    let mut at = cd_offset;
    let mut starts_file = false;
    let mut names: HashSet<&[u8]> = HashSet::with_capacity(total as usize);
    for _ in 0..total {
        if at + 46 > cd_offset + cd_size || u32_at(at) != 0x0201_4b50 {
            return Err(BAD.into());
        }
        let flags = u16_at(at + 8);
        let sizes = [u32_at(at + 20), u32_at(at + 24), u32_at(at + 42)];
        let end = at
            + 46
            + u16_at(at + 28) as usize
            + u16_at(at + 30) as usize
            + u16_at(at + 32) as usize;
        if end > cd_offset + cd_size || sizes.contains(&0xffff_ffff) {
            return Err(BAD.into());
        }
        // The zip crate decodes a name that is not UTF-8 instead of refusing it, and
        // keeps one of two entries of the same name.
        let name = &data[at + 46..at + 46 + u16_at(at + 28) as usize];
        let Ok(text) = std::str::from_utf8(name) else {
            return Err("invalid entry name".into());
        };
        if !names.insert(name) {
            return Err(format!("duplicate entry {text}"));
        }
        let kind = if u16_at(at + 4) >> 8 == 3 {
            (u32_at(at + 38) >> 16) & 0o170_000
        } else {
            0
        };
        refused.push(flags & (1 | 0x40 | 0x2000) != 0 || (kind != 0 && kind != 0o100_000));
        starts_file |= sizes[2] == 0;
        at = end;
    }
    if total > 0 && !starts_file {
        return Err(BAD.into());
    }
    Ok(refused)
}

fn read_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    refused: &[bool],
    index: usize,
    limit: u64,
) -> Result<Vec<u8>, String> {
    if refused.get(index).copied().unwrap_or(true) {
        return Err("unreadable entry".into());
    }
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
    let refused = check_layout(data)?;
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
    let manifest_bytes = read_entry(&mut zip, &refused, manifest_at, MAX_MANIFEST)?;
    let signature_text = read_entry(&mut zip, &refused, signature_at, MAX_SIGNATURE)?;
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
        crate::web_json::parse(&manifest_bytes).ok_or("unreadable manifest")?;
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
        let body = read_entry(&mut zip, &refused, at, file.size)?;
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
        // Two paths that differ only in case would collide on a case-insensitive disk.
        if !valid_path(&file.path) || !seen.insert(file.path.to_lowercase()) {
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
    if !m.files.iter().any(|f| f.path == "index.html") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_bundle::fixture::*;

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
            ".hidden",
            "a/.well-known/x",
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
}
