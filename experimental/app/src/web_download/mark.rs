//! The mark a downloaded file gets, as a browser gives it: the file came from
//! the internet (other members' uploads among them), so the system warns
//! before it runs or opens it as such. Windows: the `Zone.Identifier` stream
//! with zone 3; macOS: the `com.apple.quarantine` attribute.
use std::path::Path;

/// The text of the `Zone.Identifier` stream.
#[cfg(any(target_os = "windows", test))]
pub const ZONE_IDENTIFIER: &str = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n";

/// The value of `com.apple.quarantine` for a file downloaded at `seconds`
/// since the epoch: the flag of a download, the time in hex, the agent.
#[cfg(any(target_os = "macos", test))]
pub fn quarantine_value(seconds: u64) -> String {
    format!("0081;{seconds:08x};Mnema Desktop DEV;")
}

/// Marks `path`. A disk that cannot hold the mark (FAT, a network share)
/// leaves the file unmarked, as browsers do.
pub fn mark(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let _ = std::fs::write(stream, ZONE_IDENTIFIER);
    }
    #[cfg(target_os = "macos")]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.apple.quarantine"])
            .arg(quarantine_value(now))
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marks_say_internet_and_download() {
        assert!(ZONE_IDENTIFIER.contains("ZoneId=3"));
        assert_eq!(
            quarantine_value(0x6550_0000),
            "0081;65500000;Mnema Desktop DEV;"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_saved_file_carries_the_mark() {
        let path =
            std::env::temp_dir().join(format!("mnema-mark-{}.zip", uuid::Uuid::new_v4().simple()));
        std::fs::write(&path, b"PK").unwrap();
        mark(&path);
        #[cfg(target_os = "macos")]
        let marked = std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&path)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).starts_with("0081;"))
            .unwrap_or(false);
        #[cfg(target_os = "windows")]
        let marked = {
            let mut stream = path.as_os_str().to_owned();
            stream.push(":Zone.Identifier");
            std::fs::read_to_string(stream).is_ok_and(|t| t.contains("ZoneId=3"))
        };
        let _ = std::fs::remove_file(&path);
        assert!(marked);
    }
}
