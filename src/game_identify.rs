//! Which game, if any, is this foreground program? Three sources, in order:
//!
//! 1. names the user added in the gaming settings,
//! 2. the install folders of the launcher libraries (the name is the launcher's),
//! 3. our own list of well-known executables.
//!
//! The decision uses the image path (for the library) and the file name (for the
//! lists); the window title is read only for Java runtimes. Nothing leaves the
//! device and nothing is injected, enumerated or hooked.

use super::{game_library::InstallIndex, game_list::GameList};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A name the user added.
    User,
    /// An install folder of a launcher library.
    Library,
    /// The bundled list.
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identified {
    pub name: String,
    pub source: Source,
}

/// Programs that live in game folders but are not the game.
const HELPERS: &[&str] = &[
    "unitycrashhandler32.exe",
    "unitycrashhandler64.exe",
    "crashreportclient.exe",
    "crashreporter.exe",
    "crashpad_handler.exe",
    "unins000.exe",
    "uninstall.exe",
    "uninstaller.exe",
    "dxsetup.exe",
    "easyanticheat_eos_setup.exe",
    "easyanticheat_setup.exe",
    "vc_redist.x64.exe",
    "vc_redist.x86.exe",
];

fn is_java(file: &str) -> bool {
    matches!(file.to_ascii_lowercase().as_str(), "javaw.exe" | "java.exe")
}

pub fn identify(
    user: &[String],
    library: &InstallIndex,
    list: &GameList,
    image: &str,
    title: &str,
) -> Option<Identified> {
    let file = image.rsplit(['\\', '/']).next().filter(|f| !f.is_empty())?;
    if HELPERS.iter().any(|h| h.eq_ignore_ascii_case(file)) {
        return None;
    }
    // Java is a shared runtime: by name it is only a Minecraft candidate, with
    // its title. A Java program inside a launcher's game folder is that game.
    let java_ok = !is_java(file) || title.to_ascii_lowercase().contains("minecraft");
    if java_ok && user.iter().any(|n| n.eq_ignore_ascii_case(file)) {
        return Some(Identified {
            name: file.to_owned(),
            source: Source::User,
        });
    }
    if let Some(name) = library.find(image) {
        return Some(Identified {
            name: name.to_owned(),
            source: Source::Library,
        });
    }
    if !java_ok {
        return None;
    }
    if is_java(file) {
        return Some(Identified {
            name: "Minecraft".to_owned(),
            source: Source::List,
        });
    }
    list.find(file).map(|name| Identified {
        name: name.to_owned(),
        source: Source::List,
    })
}

#[cfg(test)]
mod tests {
    use super::super::game_library::InstalledGame;
    use super::*;

    fn library() -> InstallIndex {
        InstallIndex::new(vec![
            InstalledGame::new(
                "Example Quest",
                r"D:\SteamLibrary\steamapps\common\Example Quest",
            )
            .unwrap(),
        ])
    }

    fn ident(user: &[&str], image: &str, title: &str) -> Option<Identified> {
        let user: Vec<String> = user.iter().map(|s| s.to_string()).collect();
        identify(&user, &library(), GameList::bundled(), image, title)
    }

    #[test]
    fn library_folders_name_the_game_as_the_launcher_does() {
        let found = ident(
            &[],
            r"D:\SteamLibrary\steamapps\common\Example Quest\bin\eq.exe",
            "",
        );
        assert_eq!(
            found,
            Some(Identified {
                name: "Example Quest".into(),
                source: Source::Library
            })
        );
        assert!(
            ident(
                &[],
                r"D:\SteamLibrary\steamapps\common\Example Quest 2\eq.exe",
                ""
            )
            .is_none()
        );
        assert!(ident(&[], r"D:\Tools\eq.exe", "").is_none());
    }

    #[test]
    fn the_bundled_list_matches_by_file_name_wherever_the_game_lives() {
        let found = ident(&[], r"E:\anywhere\at\all\Cs2.EXE", "").unwrap();
        assert_eq!(found.name, "Counter-Strike 2");
        assert_eq!(found.source, Source::List);
    }

    #[test]
    fn user_names_win_and_keep_their_own_spelling() {
        let found = ident(&["Custom.exe"], r"C:\x\custom.exe", "").unwrap();
        assert_eq!(found.source, Source::User);
        assert_eq!(found.name, "custom.exe");
        let both = ident(&["cs2.exe"], r"C:\x\cs2.exe", "").unwrap();
        assert_eq!(both.source, Source::User);
    }

    #[test]
    fn user_names_beat_the_library_and_the_library_beats_the_list() {
        // A listed executable inside a library folder is named by the launcher.
        let found = ident(
            &[],
            r"D:\SteamLibrary\steamapps\common\Example Quest\cs2.exe",
            "",
        )
        .unwrap();
        assert_eq!(found.source, Source::Library);
    }

    #[test]
    fn java_is_minecraft_by_title_only() {
        let java = r"C:\Program Files\Java\bin\javaw.exe";
        assert_eq!(
            ident(&[], java, "Minecraft 1.21").unwrap().name,
            "Minecraft"
        );
        assert!(ident(&[], java, "Some editor").is_none());
        assert!(ident(&[], java, "").is_none());
        // Even a user-listed Java runtime needs the title.
        assert!(ident(&["javaw.exe"], java, "Some editor").is_none());
        assert!(ident(&["javaw.exe"], java, "Minecraft").is_some());
        // A bundled Java inside a launcher game folder is that game.
        let bundled = r"D:\SteamLibrary\steamapps\common\Example Quest\jre\bin\javaw.exe";
        assert_eq!(ident(&[], bundled, "").unwrap().source, Source::Library);
    }

    #[test]
    fn helpers_and_empty_or_unreadable_images_are_never_games() {
        assert!(
            ident(
                &[],
                r"D:\SteamLibrary\steamapps\common\Example Quest\UnityCrashHandler64.exe",
                ""
            )
            .is_none()
        );
        assert!(ident(&["unins000.exe"], r"C:\x\unins000.exe", "").is_none());
        assert!(ident(&[], "", "").is_none());
        assert!(ident(&[], r"D:\SteamLibrary\steamapps\common\Example Quest\", "").is_none());
        assert!(ident(&[], r"C:\Windows\explorer.exe", "").is_none());
        assert!(ident(&[], "chrome.exe", "Minecraft website").is_none());
    }
}
