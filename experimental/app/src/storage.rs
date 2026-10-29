//! Where the client keeps its files. An installed copy uses the profile's
//! folders (Tauri's app data and config folders). The portable copy keeps
//! everything beside its executable, or in the folder named by
//! `MNEMA_DESKTOP_DATA_DIR`, so that unpacking, using and deleting the ZIP's
//! folder leaves nothing in the profile.
use crate::native_updates::is_portable;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::{AppHandle, Manager, Runtime};

/// The folder of a portable copy, beside the executable.
const PORTABLE_FOLDER: &str = "mnema-data";
/// Overrides the folder of a portable copy; it must be an absolute path.
const CHOSEN_FOLDER_VAR: &str = "MNEMA_DESKTOP_DATA_DIR";
/// Written into the portable folder once the files of an earlier run (which
/// kept them in the profile) were carried over; it keeps that to one time.
const ADOPTED: &str = ".adopted";

/// The root of a portable copy's files: the chosen folder when it is an
/// absolute path, else a folder beside the executable.
pub fn portable_root_for(exe: &Path, chosen: Option<&Path>) -> Option<PathBuf> {
    match chosen {
        Some(dir) if dir.is_absolute() => Some(dir.to_path_buf()),
        Some(dir) if dir.as_os_str().is_empty() => Some(exe.parent()?.join(PORTABLE_FOLDER)),
        Some(_) => None,
        None => Some(exe.parent()?.join(PORTABLE_FOLDER)),
    }
}

/// The root of the portable copy's files, prepared once. None for an installed
/// copy, and for a portable one whose folder cannot be made (it then falls back
/// to the profile, which is said on stderr rather than left silent).
fn portable_root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        if !is_portable() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let chosen = std::env::var_os(CHOSEN_FOLDER_VAR);
        let Some(root) = portable_root_for(&exe, chosen.as_deref().map(Path::new)) else {
            eprintln!("Mnema Desktop: {CHOSEN_FOLDER_VAR} must be an absolute path; using the profile.");
            return None;
        };
        match std::fs::create_dir_all(&root) {
            Ok(()) => Some(root),
            Err(why) => {
                eprintln!(
                    "Mnema Desktop: cannot use {} for the portable copy ({why}); using the profile.",
                    root.display()
                );
                None
            }
        }
    })
    .as_deref()
}

/// Settings, the server list, trust, bundles and browser storage.
pub fn data_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    match portable_root() {
        Some(root) => Some(root.join("data")),
        None => app.path().app_local_data_dir().ok(),
    }
}

/// Small settings files (gaming).
pub fn config_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    match portable_root() {
        Some(root) => Some(root.join("config")),
        None => app.path().app_config_dir().ok(),
    }
}

/// The browser engine's own folder for a webview that names none. Only a
/// portable copy sets one; an installed copy keeps the engine's default.
pub fn default_webview_dir() -> Option<PathBuf> {
    portable_root().map(|root| root.join("data"))
}

/// A webview window on the portable copy's folder (nothing changes for an
/// installed copy).
pub fn window_on_default_storage<'a, R, M>(
    builder: tauri::WebviewWindowBuilder<'a, R, M>,
) -> tauri::WebviewWindowBuilder<'a, R, M>
where
    R: Runtime,
    M: Manager<R>,
{
    match default_webview_dir() {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    }
}

/// The same for a webview inside a window.
pub fn webview_on_default_storage<R: Runtime>(
    builder: tauri::webview::WebviewBuilder<R>,
) -> tauri::webview::WebviewBuilder<R> {
    match default_webview_dir() {
        Some(dir) => builder.data_directory(dir),
        None => builder,
    }
}

/// A portable copy that ran before this folder existed kept its files in the
/// profile. The first run copies them over (the profile's files stay, an
/// installed copy may share them) so that nothing is lost. Call before anything
/// reads the folders.
pub fn adopt_profile_files<R: Runtime>(app: &AppHandle<R>) {
    let Some(root) = portable_root() else { return };
    if root.join(ADOPTED).exists() {
        return;
    }
    for (from, to) in [
        (app.path().app_local_data_dir().ok(), root.join("data")),
        (app.path().app_config_dir().ok(), root.join("config")),
    ] {
        if let Some(from) = from {
            copy_missing(&from, &to);
        }
    }
    let _ = std::fs::write(root.join(ADOPTED), b"1\n");
}

/// Copies what `to` does not have yet; a file that cannot be read (the engine
/// of a running installed copy holds some) is left out. Links are not followed.
pub fn copy_missing(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    for entry in entries.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => copy_missing(&src, &dst),
            Ok(kind) if kind.is_file() && !dst.exists() => {
                let _ = std::fs::copy(&src, &dst);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mnema-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_portable_copy_keeps_its_files_beside_the_executable() {
        let exe = Path::new(r"D:\Apps\Mnema\Mnema Desktop DEV.exe");
        assert_eq!(
            portable_root_for(exe, None),
            Some(exe.parent().unwrap().join("mnema-data"))
        );
        assert_eq!(
            portable_root_for(exe, Some(Path::new(""))),
            Some(exe.parent().unwrap().join("mnema-data"))
        );
    }

    #[test]
    fn a_chosen_folder_must_be_absolute() {
        let exe = Path::new("/apps/mnema/client");
        let chosen = std::env::temp_dir();
        assert_eq!(portable_root_for(exe, Some(&chosen)), Some(chosen));
        assert_eq!(
            portable_root_for(exe, Some(Path::new("relative/dir"))),
            None
        );
        assert_eq!(
            portable_root_for(Path::new("client"), Some(Path::new(""))),
            Some(PathBuf::from("mnema-data"))
        );
    }

    #[test]
    fn the_files_of_an_earlier_run_are_copied_and_never_overwrite() {
        let base = scratch("copy");
        let (from, to) = (base.join("profile"), base.join("portable"));
        std::fs::create_dir_all(from.join("web-trust/deep")).unwrap();
        std::fs::write(from.join("instances.json"), b"old").unwrap();
        std::fs::write(from.join("web-trust/deep/state"), b"trust").unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("instances.json"), b"new").unwrap();

        copy_missing(&from, &to);

        assert_eq!(std::fs::read(to.join("instances.json")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(to.join("web-trust/deep/state")).unwrap(),
            b"trust"
        );
        assert!(
            from.join("instances.json").exists(),
            "the profile's files stay"
        );
        copy_missing(&base.join("missing"), &base.join("nowhere"));
        assert!(!base.join("nowhere").exists());
    }

    /// Every folder the client writes to goes through this module; a direct
    /// call would put a portable copy's files into the profile again.
    #[test]
    fn nothing_else_asks_tauri_for_the_profile_folders() {
        let sources = [
            ("main.rs", include_str!("main.rs")),
            ("native_updates.rs", include_str!("native_updates.rs")),
            ("web_client.rs", include_str!("web_client.rs")),
            ("web_gaming.rs", include_str!("web_gaming.rs")),
            ("web_permissions.rs", include_str!("web_permissions.rs")),
            ("web_shell.rs", include_str!("web_shell.rs")),
            ("instances.rs", include_str!("instances.rs")),
        ];
        for (name, source) in sources {
            let code = source.split("#[cfg(test)]").next().unwrap_or(source);
            for call in [
                "app_local_data_dir",
                "app_config_dir",
                "app_data_dir",
                "app_cache_dir",
                "app_log_dir",
            ] {
                assert!(!code.contains(call), "{name} calls {call}");
            }
        }
    }
}
