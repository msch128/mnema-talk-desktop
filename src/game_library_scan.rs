//! Reads the launchers' install manifests, read-only, into an [`InstallIndex`].
//!
//! Scans run on their own thread: once at start, then on request and every
//! `every` (a wait on a condition variable, not a poll). The foreground path
//! only ever reads the finished index, so a slow disk or a huge library never
//! delays game detection.
//!
//! Supported (Windows only for now): Steam, Epic Games Store, GOG Galaxy and the
//! EA app (Windows registry). Battle.net is not read: its install list
//! lives in a binary database (`product.db`) that has no public format, and
//! reading it would mean depending on private details. Games from Battle.net
//! are still recognised by the bundled list or a user-added name.

use super::game_library::{
    InstallIndex, InstalledGame, epic_game, safe_disk_path, steam_app, steam_game, steam_libraries,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_MANIFESTS: usize = 4000;

/// A source of installed games. One implementation per launcher.
pub trait Launcher: Send {
    fn name(&self) -> &'static str;
    fn scan(&self) -> Vec<InstalledGame>;
}

fn read_small(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    String::from_utf8(fs::read(path).ok()?).ok()
}

/// Files directly in `dir` whose names satisfy `wanted`, capped.
fn files_in(dir: &Path, wanted: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(&wanted))
        .map(|e| e.path())
        .take(MAX_MANIFESTS)
        .collect()
}

pub struct Steam {
    /// Steam installation folders; each is also a library.
    pub roots: Vec<PathBuf>,
}

impl Launcher for Steam {
    fn name(&self) -> &'static str {
        "Steam"
    }

    fn scan(&self) -> Vec<InstalledGame> {
        let mut libraries: Vec<String> = Vec::new();
        for root in &self.roots {
            let root_text = root.to_string_lossy().into_owned();
            // Nothing from a manifest or the registry is opened before it is
            // known to be a local path (no UNC or device path, no `..`).
            if !safe_disk_path(&root_text) {
                continue;
            }
            if !libraries.contains(&root_text) {
                libraries.push(root_text);
            }
            if let Some(text) = read_small(&root.join("steamapps").join("libraryfolders.vdf")) {
                for library in steam_libraries(&text) {
                    if safe_disk_path(&library) && !libraries.contains(&library) {
                        libraries.push(library);
                    }
                }
            }
        }
        let mut games = Vec::new();
        for library in &libraries {
            let steamapps = Path::new(library).join("steamapps");
            for manifest in files_in(&steamapps, |n| {
                n.starts_with("appmanifest_") && n.ends_with(".acf")
            }) {
                if let Some(text) = read_small(&manifest)
                    && let Some((name, installdir)) = steam_app(&text)
                    && let Some(game) = steam_game(library, &name, &installdir)
                {
                    games.push(game);
                }
            }
        }
        games
    }
}

pub struct Epic {
    pub manifests: PathBuf,
}

impl Launcher for Epic {
    fn name(&self) -> &'static str {
        "Epic Games Store"
    }

    fn scan(&self) -> Vec<InstalledGame> {
        if !safe_disk_path(&self.manifests.to_string_lossy()) {
            return Vec::new();
        }
        files_in(&self.manifests, |n| n.ends_with(".item"))
            .iter()
            .filter_map(|p| read_small(p))
            .filter_map(|text| epic_game(&text))
            .collect()
    }
}

#[cfg(windows)]
mod registry {
    //! Read-only registry access for launcher install lists.
    use super::super::wide::wide;
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
            RegQueryValueExW,
        },
    };

    pub struct Key(HKEY);

    impl Key {
        pub fn open_machine(path: &str) -> Option<Self> {
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: a NUL-terminated path and a valid out-pointer.
            let status = unsafe {
                RegOpenKeyExW(
                    HKEY_LOCAL_MACHINE,
                    wide(path).as_ptr(),
                    0,
                    KEY_READ,
                    &mut key,
                )
            };
            (status == ERROR_SUCCESS).then_some(Self(key))
        }

        pub fn subkeys(&self) -> Vec<String> {
            let mut names = Vec::new();
            for index in 0..512u32 {
                let mut buffer = [0u16; 256];
                let mut length = buffer.len() as u32;
                // SAFETY: a sized buffer and length; the optional outputs are null.
                let status = unsafe {
                    RegEnumKeyExW(
                        self.0,
                        index,
                        buffer.as_mut_ptr(),
                        &mut length,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                names.push(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            names
        }

        pub fn text(&self, name: &str) -> Option<String> {
            let mut kind = 0u32;
            let mut bytes = 0u32;
            let name = wide(name);
            // SAFETY: first call only asks for the size.
            let size = unsafe {
                RegQueryValueExW(
                    self.0,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut kind,
                    std::ptr::null_mut(),
                    &mut bytes,
                )
            };
            // REG_SZ only; expandable strings are not followed.
            if size != ERROR_SUCCESS || kind != 1 || bytes == 0 || bytes > 32768 {
                return None;
            }
            let mut data = vec![0u16; (bytes as usize).div_ceil(2)];
            let mut capacity = (data.len() * 2) as u32;
            // SAFETY: `data` holds `capacity` bytes.
            let read = unsafe {
                RegQueryValueExW(
                    self.0,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    &mut kind,
                    data.as_mut_ptr().cast(),
                    &mut capacity,
                )
            };
            if read != ERROR_SUCCESS {
                return None;
            }
            let used = (capacity as usize / 2).min(data.len());
            let end = data[..used].iter().position(|c| *c == 0).unwrap_or(used);
            Some(String::from_utf16_lossy(&data[..end]))
        }

        pub fn child(&self, name: &str) -> Option<Self> {
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: a NUL-terminated name and a valid out-pointer.
            let status =
                unsafe { RegOpenKeyExW(self.0, wide(name).as_ptr(), 0, KEY_READ, &mut key) };
            (status == ERROR_SUCCESS).then_some(Self(key))
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: this wrapper owns one open key.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}

/// GOG Galaxy writes one key per installed game with `gameName` and `path`.
#[cfg(windows)]
pub struct Gog;

#[cfg(windows)]
impl Launcher for Gog {
    fn name(&self) -> &'static str {
        "GOG Galaxy"
    }

    fn scan(&self) -> Vec<InstalledGame> {
        let Some(games) = registry::Key::open_machine(r"SOFTWARE\WOW6432Node\GOG.com\Games") else {
            return Vec::new();
        };
        games
            .subkeys()
            .iter()
            .filter_map(|id| games.child(id))
            .filter_map(|key| InstalledGame::new(&key.text("gameName")?, &key.text("path")?))
            .collect()
    }
}

/// The EA app (and Origin before it) records `Install Dir` per game.
#[cfg(windows)]
pub struct Ea;

#[cfg(windows)]
impl Launcher for Ea {
    fn name(&self) -> &'static str {
        "EA app"
    }

    fn scan(&self) -> Vec<InstalledGame> {
        [r"SOFTWARE\WOW6432Node\EA Games", r"SOFTWARE\EA Games"]
            .iter()
            .filter_map(|path| registry::Key::open_machine(path))
            .flat_map(|games| {
                games
                    .subkeys()
                    .into_iter()
                    .filter_map(|name| {
                        let key = games.child(&name)?;
                        InstalledGame::new(&name, &key.text("Install Dir")?)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// The launchers this system can have.
pub fn default_launchers() -> Vec<Box<dyn Launcher>> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut launchers: Vec<Box<dyn Launcher>> = Vec::new();
    #[cfg(windows)]
    {
        let mut roots = vec![PathBuf::from(r"C:\Program Files (x86)\Steam")];
        if let Some(key) = registry::Key::open_machine(r"SOFTWARE\WOW6432Node\Valve\Steam")
            && let Some(path) = key.text("InstallPath")
        {
            roots.insert(0, PathBuf::from(path));
        }
        launchers.push(Box::new(Steam { roots }));
        if let Some(data) = std::env::var_os("ProgramData") {
            launchers.push(Box::new(Epic {
                manifests: Path::new(&data)
                    .join("Epic")
                    .join("EpicGamesLauncher")
                    .join("Data")
                    .join("Manifests"),
            }));
        }
        launchers.push(Box::new(Gog));
        launchers.push(Box::new(Ea));
    }
    launchers
}

/// The finished index, shared with the foreground path.
#[derive(Default)]
pub struct Library {
    index: RwLock<Arc<InstallIndex>>,
    wake: (Mutex<bool>, Condvar),
    stop: AtomicBool,
}

impl Library {
    pub fn current(&self) -> Arc<InstallIndex> {
        self.index.read().map(|i| i.clone()).unwrap_or_default()
    }

    pub fn replace(&self, index: InstallIndex) {
        if let Ok(mut slot) = self.index.write() {
            *slot = Arc::new(index);
        }
    }

    /// Ask the refresher for a scan now, without waiting for it.
    pub fn rescan_now(&self) {
        if let Ok(mut pending) = self.wake.0.lock() {
            *pending = true;
            self.wake.1.notify_all();
        }
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.rescan_now();
    }
}

/// Scans every launcher once into one index.
pub fn scan_all(launchers: &[Box<dyn Launcher>]) -> InstallIndex {
    InstallIndex::new(launchers.iter().flat_map(|l| l.scan()).collect())
}

/// Starts the refresher thread: scan now, then on request and every `every`.
pub fn start_refresher(
    library: Arc<Library>,
    launchers: Vec<Box<dyn Launcher>>,
    every: Duration,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("mnema-game-library".into())
        .spawn(move || {
            while !library.stop.load(Ordering::SeqCst) {
                library.replace(scan_all(&launchers));
                let Ok(mut pending) = library.wake.0.lock() else {
                    return;
                };
                if !*pending {
                    pending = match library.wake.1.wait_timeout(pending, every) {
                        Ok((guard, _)) => guard,
                        Err(_) => return,
                    };
                }
                *pending = false;
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A throwaway directory removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mnema-library-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, relative: &str, text: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn acf(name: &str, installdir: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"name\"\t\"{name}\"\n\t\"installdir\"\t\"{installdir}\"\n}}\n"
        )
    }

    #[test]
    fn steam_scan_follows_libraryfolders_and_reads_every_manifest() {
        let steam = Dir::new("steam");
        let other = Dir::new("steam-other");
        let other_path = other.0.to_string_lossy().replace('\\', "\\\\");
        steam.write(
            "steamapps/libraryfolders.vdf",
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\"{other_path}\"\n\t}}\n}}\n"
            ),
        );
        steam.write("steamapps/appmanifest_1.acf", &acf("Alpha", "Alpha"));
        other.write("steamapps/appmanifest_2.acf", &acf("Beta", "Beta Game"));
        other.write(
            "steamapps/appmanifest_3.acf",
            &acf("Proton 9", "Proton 9.0"),
        );
        other.write("steamapps/appmanifest_4.acf", "garbage");
        other.write("steamapps/notes.txt", "ignored");
        let index = scan_all(&[Box::new(Steam {
            roots: vec![steam.0.clone(), steam.0.join("missing")],
        })]);
        assert_eq!(index.len(), 2);
        let alpha = steam.0.join("steamapps/common/Alpha/alpha.exe");
        let beta = other.0.join("steamapps/common/Beta Game/bin/beta.exe");
        assert_eq!(index.find(&alpha.to_string_lossy()), Some("Alpha"));
        assert_eq!(index.find(&beta.to_string_lossy()), Some("Beta"));
    }

    #[test]
    fn epic_scan_reads_item_files_only() {
        let manifests = Dir::new("epic");
        manifests.write(
            "A.item",
            include_str!("../fixtures/gaming/epic-example.item"),
        );
        manifests.write("B.txt", "{}");
        manifests.write("C.item", "not json");
        let games = Epic {
            manifests: manifests.0.clone(),
        }
        .scan();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Example Racer");
    }

    #[test]
    fn oversized_manifests_are_ignored() {
        let steam = Dir::new("big");
        let padding = " ".repeat(MAX_MANIFEST_BYTES as usize);
        steam.write(
            "steamapps/appmanifest_1.acf",
            &format!("{}{padding}", acf("Big", "Big")),
        );
        assert!(
            Steam {
                roots: vec![steam.0.clone()]
            }
            .scan()
            .is_empty()
        );
    }

    struct Fake(Mutex<Vec<InstalledGame>>);

    impl Launcher for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn scan(&self) -> Vec<InstalledGame> {
            self.0.lock().unwrap().clone()
        }
    }

    #[test]
    fn refresher_scans_at_start_and_again_on_request_but_not_on_a_timer_tick() {
        let game = |n: &str, d: &str| InstalledGame::new(n, d).unwrap();
        let library = Arc::new(Library::default());
        assert!(library.current().is_empty());
        // A launcher whose answer changes between scans.
        struct Changing(mpsc::Sender<()>, Mutex<u32>);
        impl Launcher for Changing {
            fn name(&self) -> &'static str {
                "changing"
            }
            fn scan(&self) -> Vec<InstalledGame> {
                let mut count = self.1.lock().unwrap();
                *count += 1;
                let dir = format!("/games/g{count}");
                let _ = self.0.send(());
                vec![InstalledGame::new(&format!("G{count}"), &dir).unwrap()]
            }
        }
        let (tx, rx) = mpsc::channel();
        let handle = start_refresher(
            library.clone(),
            vec![
                Box::new(Changing(tx, Mutex::new(0))),
                Box::new(Fake(Mutex::new(vec![game("Fixed", "/games/fixed")]))),
            ],
            Duration::from_secs(3600),
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The first scan has finished once the index holds both launchers' games.
        for _ in 0..200 {
            if library.current().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(library.current().find("/games/g1/x.exe"), Some("G1"));
        assert_eq!(library.current().find("/games/fixed/x.exe"), Some("Fixed"));
        library.rescan_now();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..200 {
            if library.current().find("/games/g2/x.exe").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(library.current().find("/games/g2/x.exe"), Some("G2"));
        assert_eq!(library.current().find("/games/g1/x.exe"), None);
        library.shutdown();
        handle.join().unwrap();
    }
}
