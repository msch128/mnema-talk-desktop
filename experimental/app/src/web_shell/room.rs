//! Whether one more server may run: the browser storage of two servers has to
//! be kept apart, and some systems cannot.
/// Whether the browser storage of two servers can be kept apart. WebView2
/// does it with a folder per server; WKWebView needs macOS 14 for a data
/// store of its own. Where it cannot, a second server is not started rather
/// than sharing storage (and the end-to-end keys in it) with the first.
pub(super) fn storage_isolated() -> bool {
    #[cfg(target_os = "macos")]
    {
        static MAJOR: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
        data_stores_available(*MAJOR.get_or_init(|| {
            let out = std::process::Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()?;
            macos_major(&String::from_utf8_lossy(&out.stdout))
        }))
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// WKWebView's data store API (creating, listing and removing stores) exists
/// from macOS 14. An unknown version is treated as too old.
#[cfg(any(target_os = "macos", test))]
fn data_stores_available(major: Option<u32>) -> bool {
    major.is_some_and(|major| major >= 14)
}

#[cfg(any(target_os = "macos", test))]
fn macos_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// May one more server run next to those that do? The first always may.
pub(super) fn room_for(running: usize) -> bool {
    room_with(running, storage_isolated())
}

fn room_with(running: usize, isolated: bool) -> bool {
    running == 0 || isolated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_versions_are_read_from_the_system() {
        assert_eq!(macos_major("14.6.1\n"), Some(14));
        assert_eq!(macos_major("26.0"), Some(26));
        assert_eq!(macos_major("12"), Some(12));
        assert_eq!(macos_major(""), None);
        assert_eq!(macos_major("x.1"), None);
    }

    #[test]
    fn the_data_store_api_needs_macos_14() {
        assert!(!data_stores_available(Some(12)));
        assert!(!data_stores_available(Some(13)));
        assert!(data_stores_available(Some(14)));
        assert!(data_stores_available(Some(26)));
        assert!(!data_stores_available(None));
    }

    #[test]
    fn the_first_server_always_runs_and_more_need_separate_storage() {
        assert!(room_for(0));
        assert!(room_with(0, false));
        assert!(!room_with(1, false));
        assert!(room_with(1, true));
    }
}
