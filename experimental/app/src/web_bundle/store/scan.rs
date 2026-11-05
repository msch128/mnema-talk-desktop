//! Loading: verifies the built-in bundle and the files of the cache directory.
use super::{MAX_CACHED_FILES, MAX_ZIP};
use crate::web_bundle::{
    Bundle, Held, Source, TrustedKey, Version,
    verify::{NOT_TRUSTED, verify},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// A cached file that could not be read (a sharing lock, a scan): it may be
/// fine next start, so it is kept.
const UNREADABLE: &str = "unreadable";

/// What loading found.
pub(super) struct Scan {
    pub(super) verified: Vec<Held>,
    /// Names of the files that failed verification.
    pub(super) rejected: Vec<String>,
    /// A bundle was refused only because its signer is not trusted now.
    pub(super) withdrawn: bool,
}

/// Verifies the built-in bundle and everything in the cache directory.
pub(super) fn scan(dir: Option<&Path>, builtin: Option<&[u8]>, keys: &[TrustedKey]) -> Scan {
    let mut scan = Scan {
        verified: Vec::new(),
        rejected: Vec::new(),
        withdrawn: false,
    };
    if let Some(bytes) = builtin {
        scan.add_builtin(bytes, keys);
    }
    let mut usable = 0;
    for (named, path) in cached_files(dir) {
        if usable >= MAX_CACHED_FILES {
            break;
        }
        if scan.add_cached(named, &path, keys) {
            usable += 1;
        }
    }
    scan
}

impl Scan {
    fn add_builtin(&mut self, bytes: &[u8], keys: &[TrustedKey]) {
        match verify(bytes, keys) {
            Ok(bundle) => self.verified.push(Held {
                bundle: Arc::new(bundle),
                source: Source::Builtin,
            }),
            Err(reason) => {
                self.withdrawn |= reason == NOT_TRUSTED;
                self.rejected.push("built-in".to_owned())
            }
        }
    }

    /// Takes one cached file in; whether it is now a usable bundle.
    fn add_cached(&mut self, named: Version, path: &Path, keys: &[TrustedKey]) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_cached(named, path, keys) {
            Ok(bundle)
                if !self
                    .verified
                    .iter()
                    .any(|h| h.bundle.version == bundle.version) =>
            {
                self.verified.push(Held {
                    bundle: Arc::new(bundle),
                    source: Source::Cache,
                });
                return true;
            }
            // Same version as one already held: the file is redundant.
            Ok(_) => {
                let _ = std::fs::remove_file(path);
            }
            Err(reason) => {
                // A file nobody can use is removed; one whose signer is
                // not trusted right now stays, the key may return, and so
                // does one that could not be read, a lock may be gone.
                if reason == NOT_TRUSTED {
                    self.withdrawn = true;
                } else if reason != UNREADABLE {
                    let _ = std::fs::remove_file(path);
                }
                self.rejected.push(name);
            }
        }
        false
    }
}

/// A cached file, verified and checked against the version in its name.
fn read_cached(named: Version, path: &Path, keys: &[TrustedKey]) -> Result<Bundle, String> {
    let meta = std::fs::metadata(path).map_err(|_| UNREADABLE.to_owned())?;
    if meta.len() > MAX_ZIP {
        return Err("file too large".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|_| UNREADABLE.to_owned())?;
    let bundle = verify(&bytes, keys)?;
    if bundle.version == named {
        Ok(bundle)
    } else {
        Err("file name does not match the bundle".to_owned())
    }
}

/// The cached bundle files by the version in their name, newest first. A
/// `.partial` left by an interrupted install is removed.
pub(super) fn cached_files(dir: Option<&Path>) -> Vec<(Version, PathBuf)> {
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
