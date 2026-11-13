//! The portable copy replaces itself from the signed portable ZIP of a release.
//!
//! Only bytes whose minisign signature already verified reach this module (the
//! plugin's `download` verifies before it returns them). The ZIP is unpacked
//! into a staging folder beside the program; then the running program is
//! renamed aside (Windows lets a running executable be renamed, not deleted),
//! the new one takes its place, and the other files of the ZIP replace theirs.
//! The data folder `mnema-data` is never touched, so the servers, the sign-ins
//! and the settings stay. The renamed old program is deleted at the next start.
//! If the new program cannot be put in place, the old one is moved back and
//! nothing has changed.
use super::logic::{MAX_INSTALLER, PORTABLE_MARKER};
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

/// Where the new files are unpacked before they replace the old ones.
pub const STAGE: &str = ".mnema-update";
/// The suffix of the program that was replaced; deleted at the next start.
pub const OLD_SUFFIX: &str = ".old";
/// The data folder of a portable copy (`storage.rs`), never part of an update.
const DATA_FOLDER: &str = "mnema-data";
/// A release ZIP has a few dozen files.
const MAX_ENTRIES: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum PortableError {
    /// The ZIP is not a portable release: no program, no marker, a path that
    /// leaves the folder, a link, or too large.
    Package,
    /// The folder of the program cannot be written.
    Folder,
    /// A file could not be put in place.
    Swap,
}

/// One file of the new ZIP, relative to the program's folder.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub index: usize,
    pub path: PathBuf,
    /// The program itself: it replaces the running executable, whatever its name.
    pub program: bool,
}

fn relative(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    let normal = path.components().all(|c| matches!(c, Component::Normal(_)));
    (normal && !name.contains('\\') && !name.is_empty()).then(|| path.to_path_buf())
}

/// The files of a portable release ZIP and which one is the program. Refuses
/// anything a release never contains.
pub fn plan(bytes: &[u8]) -> Result<Vec<Entry>, PortableError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| PortableError::Package)?;
    if zip.len() > MAX_ENTRIES {
        return Err(PortableError::Package);
    }
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    let mut marker = false;
    for index in 0..zip.len() {
        let file = zip.by_index(index).map_err(|_| PortableError::Package)?;
        if file.is_dir() {
            continue;
        }
        if file.is_symlink() || !file.is_file() {
            return Err(PortableError::Package);
        }
        let name = file.name().map_err(|_| PortableError::Package)?;
        let path = relative(&name).ok_or(PortableError::Package)?;
        if path.starts_with(DATA_FOLDER) || path.starts_with(STAGE) {
            return Err(PortableError::Package);
        }
        total = total.saturating_add(file.size());
        if total > MAX_INSTALLER {
            return Err(PortableError::Package);
        }
        let top = path.components().count() == 1;
        marker |= top && path.as_os_str() == PORTABLE_MARKER;
        let program = top
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        entries.push(Entry {
            index,
            path,
            program,
        });
    }
    let programs = entries.iter().filter(|e| e.program).count();
    if programs != 1 || !marker {
        return Err(PortableError::Package);
    }
    Ok(entries)
}

fn unpack(bytes: &[u8], entries: &[Entry], stage: &Path) -> Result<(), PortableError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| PortableError::Package)?;
    for entry in entries {
        let mut file = zip
            .by_index(entry.index)
            .map_err(|_| PortableError::Package)?;
        let mut body = Vec::new();
        // The announced size was checked; a lying entry is cut off and refused.
        file.by_ref()
            .take(MAX_INSTALLER + 1)
            .read_to_end(&mut body)
            .map_err(|_| PortableError::Package)?;
        if body.len() as u64 != file.size() {
            return Err(PortableError::Package);
        }
        let target = stage.join(&entry.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|_| PortableError::Folder)?;
        }
        std::fs::write(&target, body).map_err(|_| PortableError::Folder)?;
    }
    Ok(())
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(OLD_SUFFIX);
    exe.with_file_name(name)
}

/// Replaces the portable copy whose program is `exe` with the verified ZIP.
pub fn replace(bytes: &[u8], exe: &Path) -> Result<(), PortableError> {
    let entries = plan(bytes)?;
    let dir = exe.parent().ok_or(PortableError::Folder)?;
    let stage = dir.join(STAGE);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir(&stage).map_err(|_| PortableError::Folder)?;
    let result = unpack(bytes, &entries, &stage).and_then(|()| swap(&entries, &stage, exe, dir));
    let _ = std::fs::remove_dir_all(&stage);
    result
}

fn swap(entries: &[Entry], stage: &Path, exe: &Path, dir: &Path) -> Result<(), PortableError> {
    // The program first: if it cannot be put in place, nothing has changed.
    let program = entries
        .iter()
        .find(|e| e.program)
        .ok_or(PortableError::Package)?;
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|_| PortableError::Folder)?;
    if std::fs::rename(stage.join(&program.path), exe).is_err() {
        let _ = std::fs::rename(&old, exe);
        return Err(PortableError::Swap);
    }
    // The notices and licences beside it; none of them is in use. The new
    // program is in place already, so one that cannot be replaced stays old.
    for entry in entries.iter().filter(|e| !e.program) {
        let target = dir.join(&entry.path);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(stage.join(&entry.path), &target);
    }
    Ok(())
}

/// After an update: deletes the replaced program and a staging folder that
/// an interrupted update left behind. True once the replaced program is gone.
pub fn clean_up(exe: &Path) -> bool {
    if let Some(dir) = exe.parent() {
        let _ = std::fs::remove_dir_all(dir.join(STAGE));
    }
    let old = old_path(exe);
    let _ = std::fs::remove_file(&old);
    !old.exists()
}

/// The replaced program may still be running for a moment while the new one
/// starts: try again for a while, off the start path.
pub fn clean_up_soon(exe: PathBuf) {
    std::thread::spawn(move || {
        for _ in 0..60 {
            if clean_up(&exe) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

/// Whether the program that an update replaced is still beside `exe`.
#[cfg(feature = "update-selftest")]
pub fn replaced_left(exe: &Path) -> bool {
    old_path(exe).exists()
}
