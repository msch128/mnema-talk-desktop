//! The mark a downloaded file gets, as a browser gives it: the file came from
//! the internet (other members' uploads among them), so the system warns
//! before it runs or opens it as such. Windows: the `Zone.Identifier` stream
//! with zone 3; macOS: the `com.apple.quarantine` attribute, naming the app
//! (`agent`, the product name of the package) as the one that downloaded it.
use std::path::Path;

/// The text of the `Zone.Identifier` stream.
#[cfg(any(target_os = "windows", test))]
pub const ZONE_IDENTIFIER: &str = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n";

/// The value of `com.apple.quarantine` for a file `agent` downloaded at
/// `seconds` since the epoch: the flag of a download, the time in hex, the
/// agent (without the `;` that separates the fields).
#[cfg(any(target_os = "macos", test))]
pub fn quarantine_value(seconds: u64, agent: &str) -> String {
    let agent: String = agent
        .chars()
        .filter(|c| *c != ';' && !c.is_control())
        .collect();
    format!("0081;{seconds:08x};{agent};")
}

/// Marks `path`. A disk that cannot hold the mark (FAT, a network share)
/// leaves the file unmarked, as browsers do. A hard link to a marked file
/// shares the mark.
pub fn mark(path: &Path, agent: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = agent;
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(stream, ZONE_IDENTIFIER);
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let value = quarantine_value(now, agent);
        let (Ok(file), Ok(name)) = (
            std::ffi::CString::new(path.as_os_str().as_bytes()),
            std::ffi::CString::new("com.apple.quarantine"),
        ) else {
            return;
        };
        // SAFETY: both strings are NUL-terminated and live across the call;
        // the value is read for exactly its length. XATTR_NOFOLLOW: a link
        // put at the path (the webview's own downloads land where it was told)
        // is marked itself, never the file it points to.
        let _ = unsafe {
            libc::setxattr(
                file.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = (path, agent);
}

/// Whether `path` carries the mark (for the tests of the paths that set it).
#[cfg(test)]
pub fn marked(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(path)
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("0081;"))
    }
    #[cfg(target_os = "windows")]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        std::fs::read_to_string(stream).is_ok_and(|t| t.contains("ZoneId=3"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = path;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marks_say_internet_download_and_who() {
        assert!(ZONE_IDENTIFIER.contains("ZoneId=3"));
        assert_eq!(
            quarantine_value(0x6550_0000, "Mnema Desktop"),
            "0081;65500000;Mnema Desktop;"
        );
        assert_eq!(quarantine_value(1, "a;b\nc"), "0081;00000001;abc;");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_link_at_the_path_is_never_followed() {
        let dir =
            std::env::temp_dir().join(format!("mnema-mark-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let elsewhere = dir.join("someone-elses.txt");
        std::fs::write(&elsewhere, b"x").unwrap();
        let link = dir.join("download.zip");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        mark(&link, "Mnema test");
        let followed = marked(&elsewhere);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!followed, "the file behind the link was marked");
    }

    /// libc is needed for setxattr only, so only macOS builds depend on it.
    #[test]
    fn libc_is_a_dependency_of_the_macos_build_only() {
        let manifest = include_str!("../../Cargo.toml");
        let mut table = "";
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                table = line;
            } else if line.starts_with("libc") {
                assert_eq!(
                    table, "[target.'cfg(target_os=\"macos\")'.dependencies]",
                    "{line}"
                );
                return;
            }
        }
        panic!("no libc dependency");
    }

    #[test]
    fn a_saved_file_carries_the_mark() {
        let path =
            std::env::temp_dir().join(format!("mnema-mark-{}.zip", uuid::Uuid::new_v4().simple()));
        std::fs::write(&path, b"PK").unwrap();
        mark(&path, "Mnema test");
        let carries = marked(&path);
        let _ = std::fs::remove_file(&path);
        assert!(carries);
    }
}
