//! Games the user adds themselves. The only source is the application that was
//! in front last (as the foreground event announced it) when the person presses
//! "This is a game" in the gaming settings or overlay, and confirms the file
//! name they are shown. Nothing is enumerated, nothing leaves the device, and
//! the full image path never leaves the native host: only the bare file name is
//! offered, and only on that request.

use super::{foreground::Foreground, game_identify, game_list::valid_executable};

/// The most names the user list holds (the same bound the settings enforce).
pub const MAX_GAMES: usize = 32;

/// The file name worth offering for `front`, if any: not Mnema itself, a valid
/// bare `.exe` name, not a shared runtime (Java is matched by title only) and
/// not a helper that lives in game folders.
pub fn candidate(front: &Foreground, own_pid: u32) -> Option<String> {
    if front.pid == 0 || front.pid == own_pid {
        return None;
    }
    let name = front.basename();
    (valid_executable(name) && !game_identify::is_shared_runtime(name) && !game_identify::is_helper(name))
        .then(|| name.to_owned())
}

/// The user's list with `name` added. A name already there (any case) is kept
/// once; an invalid name or a full list is an error.
pub fn add_game(games: &[String], name: &str) -> Result<Vec<String>, String> {
    if !valid_executable(name) {
        return Err("Enter the game's executable file name, for example MyGame.exe.".into());
    }
    if game_identify::is_shared_runtime(name) {
        return Err("Java and similar runtimes cannot be added as a game.".into());
    }
    let mut next = games.to_vec();
    if !next.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        if next.len() >= MAX_GAMES {
            return Err(format!("At most {MAX_GAMES} games can be added."));
        }
        next.push(name.to_owned());
    }
    Ok(next)
}

/// The list without `name` (any case).
pub fn remove_game(games: &[String], name: &str) -> Vec<String> {
    games
        .iter()
        .filter(|n| !n.eq_ignore_ascii_case(name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front(pid: u32, image: &str) -> Foreground {
        Foreground {
            window: 1,
            pid,
            image: image.into(),
        }
    }

    #[test]
    fn the_app_in_front_is_offered_by_file_name_only() {
        assert_eq!(
            candidate(&front(7, r"D:\Games\My Game\MyGame.exe"), 1).as_deref(),
            Some("MyGame.exe")
        );
        assert_eq!(
            candidate(&front(7, "C:/x/game64.EXE"), 1).as_deref(),
            Some("game64.EXE")
        );
    }

    #[test]
    fn mnema_itself_unknown_programs_runtimes_and_helpers_are_not_offered() {
        assert_eq!(candidate(&front(1, r"C:\Mnema\mnema.exe"), 1), None);
        assert_eq!(candidate(&front(0, r"C:\x\a.exe"), 1), None);
        assert_eq!(candidate(&front(7, ""), 1), None, "image refused by the system");
        assert_eq!(candidate(&front(7, r"C:\x\readme.txt"), 1), None);
        assert_eq!(candidate(&front(7, r"C:\Java\bin\javaw.exe"), 1), None);
        assert_eq!(candidate(&front(7, r"C:\g\crashpad_handler.exe"), 1), None);
        assert_eq!(candidate(&front(7, r"C:\g\a b\..\x.exe"), 1).as_deref(), Some("x.exe"));
    }

    #[test]
    fn adding_keeps_one_entry_per_name_and_checks_the_name() {
        let list = vec!["Wow.exe".to_string()];
        let added = add_game(&list, "MyGame.exe").unwrap();
        assert_eq!(added, ["Wow.exe", "MyGame.exe"]);
        assert_eq!(add_game(&added, "mygame.EXE").unwrap(), added, "no duplicate");
        for bad in ["", "MyGame", "..\\a.exe", "a/b.exe", "javaw.exe", "JAVA.EXE", ".exe"] {
            assert!(add_game(&list, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_list_is_bounded() {
        let full: Vec<String> = (0..MAX_GAMES).map(|i| format!("g{i}.exe")).collect();
        assert!(add_game(&full, "extra.exe").is_err());
        assert_eq!(add_game(&full, "G1.exe").unwrap(), full, "a known name still succeeds");
    }

    #[test]
    fn removing_ignores_case_and_leaves_the_rest() {
        let list = vec!["A.exe".to_string(), "b.exe".to_string()];
        assert_eq!(remove_game(&list, "a.EXE"), ["b.exe"]);
        assert_eq!(remove_game(&list, "c.exe"), list);
        assert!(remove_game(&["a.exe".to_string()], "A.exe").is_empty());
    }
}
