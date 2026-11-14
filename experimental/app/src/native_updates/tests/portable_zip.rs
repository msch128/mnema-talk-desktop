use super::*;

#[test]
fn a_portable_release_zip_names_one_program_and_carries_the_marker() {
    let entries = plan(&portable_zip(b"new")).unwrap();
    let programs: Vec<_> = entries.iter().filter(|e| e.program).collect();
    assert_eq!(programs.len(), 1);
    assert_eq!(programs[0].path, PathBuf::from(PORTABLE_EXE));
    assert_eq!(entries.len(), 4);
}

#[test]
fn a_zip_that_is_not_a_portable_release_is_refused() {
    let marker: (&str, &[u8]) = (PORTABLE_MARKER, b"portable\n");
    let exe: (&str, &[u8]) = (PORTABLE_EXE, b"MZ");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not a zip", b"MZ not a zip".to_vec()),
        ("no program", zip_of(&[marker])),
        ("no marker", zip_of(&[exe])),
        ("two programs", zip_of(&[exe, marker, ("Other.exe", b"MZ")])),
        (
            "program in a folder only",
            zip_of(&[marker, ("bin/App.exe", b"MZ")]),
        ),
        (
            "leaves the folder",
            zip_of(&[exe, marker, ("../evil.dll", b"x")]),
        ),
        (
            "into the data folder",
            zip_of(&[exe, marker, ("mnema-data/data/x", b"x")]),
        ),
        (
            "into the staging folder",
            zip_of(&[exe, marker, (".mnema-update/x", b"x")]),
        ),
        (
            "a backslash path",
            zip_of(&[exe, marker, ("a\\..\\..\\evil.dll", b"x")]),
        ),
        (
            "an absolute path",
            zip_of(&[exe, marker, ("/etc/evil", b"x")]),
        ),
    ];
    for (name, bytes) in cases {
        assert_eq!(plan(&bytes), Err(PortableError::Package), "{name}");
    }
}

#[test]
fn a_link_inside_the_zip_is_refused() {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in [
        (PORTABLE_EXE, b"MZ".as_slice()),
        (PORTABLE_MARKER, b"portable\n"),
    ] {
        zip.start_file(name, options).unwrap();
        std::io::Write::write_all(&mut zip, body).unwrap();
    }
    zip.add_symlink("LICENSE", "/etc/passwd", options).unwrap();
    zip.finish().unwrap();
    assert_eq!(plan(&out.into_inner()), Err(PortableError::Package));
}

#[test]
fn a_folder_that_cannot_be_written_is_named_as_the_reason_and_changes_nothing() {
    let dir = data_dir("portable-nofolder");
    let exe = dir.join("missing").join(PORTABLE_EXE);
    assert_eq!(
        replace(&portable_zip(b"new"), &exe),
        Err(PortableError::Folder)
    );
    assert!(!dir.join("missing").exists());
}

#[test]
fn the_program_is_put_back_when_the_new_one_cannot_take_its_place() {
    let dir = data_dir("portable-rollback");
    let folder = dir.join("app");
    std::fs::create_dir_all(&folder).unwrap();
    // A program whose name the running copy has is missing: renaming it aside
    // fails, so nothing is changed and the folder is left as it was.
    let exe = folder.join(PORTABLE_EXE);
    std::fs::write(folder.join("LICENSE"), b"old").unwrap();
    assert_eq!(
        replace(&portable_zip(b"new"), &exe),
        Err(PortableError::Folder)
    );
    assert_eq!(std::fs::read(folder.join("LICENSE")).unwrap(), b"old");
    assert!(!folder.join(STAGE).exists());
}
