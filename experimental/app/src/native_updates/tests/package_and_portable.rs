use super::*;

/// The vectors in `test-vectors/` are shared with the Linux client, which
/// ports the verification to TypeScript (`linux/test/vectors.test.ts`).
#[test]
fn shared_minisign_vectors() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../test-vectors/minisign.json")).unwrap();
    for c in v["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let got = verify_package(
            c["package"].as_str().unwrap().as_bytes(),
            c["signature"].as_str().unwrap(),
            c["pubkey"].as_str().unwrap(),
            c["version"].as_str().unwrap(),
        );
        let want = match c["expect"].as_str() {
            None => Ok(()),
            Some("key") => Err(VerifyError::Key),
            Some("signature") => Err(VerifyError::Signature),
            Some("mismatch") => Err(VerifyError::Mismatch),
            Some("version") => Err(VerifyError::Version),
            Some(other) => panic!("{name}: unknown expectation {other}"),
        };
        assert_eq!(got, want, "{name}");
    }
}

/// A package signed by the release pipeline (`MNEMA_TEST_UPDATE_PACKAGE`, with
/// its `.sig` next to it and `MNEMA_TEST_UPDATE_VERSION`) against the updater
/// key in `tauri.conf.json`: the real signature, not one the tests make, and
/// every way of getting it wrong.
#[test]
fn a_package_signed_by_the_release_pipeline_verifies_and_every_wrong_form_is_refused() {
    let (Some(package), Some(version)) = (
        std::env::var_os("MNEMA_TEST_UPDATE_PACKAGE").filter(|p| std::path::Path::new(p).exists()),
        std::env::var("MNEMA_TEST_UPDATE_VERSION").ok(),
    ) else {
        return;
    };
    let mut sig_path = package.clone();
    sig_path.push(".sig");
    let bytes = std::fs::read(&package).unwrap();
    let signature = std::fs::read_to_string(sig_path).unwrap();
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
    let pubkey = conf["plugins"]["updater"]["pubkey"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(
        verify_package(&bytes, &signature, &pubkey, &version),
        Ok(())
    );
    let mut tampered = bytes.clone();
    tampered[bytes.len() / 2] ^= 1;
    assert_eq!(
        verify_package(&tampered, &signature, &pubkey, &version),
        Err(VerifyError::Mismatch)
    );
    assert_eq!(
        verify_package(&bytes[..bytes.len() - 1], &signature, &pubkey, &version),
        Err(VerifyError::Mismatch)
    );
    assert_eq!(
        verify_package(&bytes, &signature, &pubkey, "0.0.1"),
        Err(VerifyError::Version)
    );
    assert_eq!(
        verify_package(&bytes, &signature, &keys().pubkey(), &version),
        Err(VerifyError::Mismatch)
    );
}

// ------------------------------------------------------------ portable build

#[test]
fn only_a_marker_file_next_to_the_executable_makes_a_copy_portable() {
    let dir = data_dir("marker");
    let exe = dir.join("Mnema Desktop DEV.exe");
    assert!(!portable_at(&exe), "an installed copy has no marker");
    std::fs::write(dir.join(PORTABLE_MARKER), b"portable\n").unwrap();
    assert!(portable_at(&exe));
    // Elsewhere, or as a directory, the name means nothing.
    assert!(!portable_at(&dir.join("sub/Mnema Desktop DEV.exe")));
    std::fs::remove_file(dir.join(PORTABLE_MARKER)).unwrap();
    std::fs::create_dir(dir.join(PORTABLE_MARKER)).unwrap();
    assert!(!portable_at(&exe));
    assert!(!portable_at(Path::new("")));
}

#[test]
fn the_release_page_is_built_only_from_a_plain_version() {
    assert_eq!(
        release_page("1.4.2").unwrap().as_str(),
        "https://github.com/msch128/mnema-talk-desktop/releases/tag/v1.4.2"
    );
    for odd in ["", "1.4", "v1.4.2", "1.4.2/../x", "1.4.2?x=1", "1.4.2#a"] {
        assert!(release_page(odd).is_none(), "{odd}");
    }
}

#[test]
fn a_portable_copy_announces_a_release_but_never_runs_an_installer() {
    let keys = keys();
    let dir = data_dir("portable");
    let rel = release("0.2.0", &keys);
    let port = publish(&rel);
    // A leftover from before: it must neither be offered nor run.
    std::fs::write(dir.join(PENDING_FILE), &rel.bytes).unwrap();
    Persisted {
        pending: Some(Pending {
            version: "0.2.0".into(),
            signature: "x".into(),
        }),
        ..Persisted::default()
    }
    .save(Some(&dir));
    let app = app_built(&keys, port, &dir, false, true);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();
    let snap = state.snapshot();
    assert_eq!((snap.phase, snap.portable), (Phase::Idle, true));
    assert!(!dir.join(PENDING_FILE).exists());

    assert!(run(look(&handle, false)), "the new version is announced");
    let snap = state.snapshot();
    assert_eq!(
        (snap.phase, snap.version.as_deref()),
        (Phase::Available, Some("0.2.0"))
    );
    assert_eq!(
        menu_text(snap.phase, snap.portable),
        "Download update \u{2022}"
    );

    // Neither choice downloads or keeps anything.
    for mode in [Mode::Now, Mode::Later] {
        run(install(&handle, mode));
        let snap = state.snapshot();
        assert_eq!(snap.phase, Phase::Available, "{snap:?}");
        assert_eq!(snap.received, 0);
        assert!(!dir.join(PENDING_FILE).exists());
    }
    assert!(run(prepare_pending(&handle)).is_none());
    // Nor does the start-up path that applies a kept installer.
    run(apply_pending(&handle));
    assert_eq!(state.snapshot().phase, Phase::Available);
    assert_eq!(Persisted::load(Some(&dir)).pending, None);
    assert!(!dir.join(PENDING_FILE).exists());

    // Skipping the version works here too and survives a restart, so a copy
    // that is not updated stops announcing the same release at every start.
    skip(&handle);
    assert_eq!(
        Persisted::load(Some(&dir)).skipped.as_deref(),
        Some("0.2.0")
    );
}

#[test]
fn an_installed_copy_still_offers_the_installer() {
    assert_eq!(menu_text(Phase::Available, false), "Update \u{2022}");
    assert_eq!(menu_text(Phase::Idle, true), "Updates");
}

/// The release workflow is what puts the marker into the portable ZIP and keeps
/// it out of the installer; this ties its spelling to the one the program reads.
#[test]
fn the_release_workflow_marks_the_portable_zip_and_checks_the_installer_has_none() {
    let workflow = include_str!("../../../../../.github/workflows/desktop-release.yml");
    let marked = workflow
        .lines()
        .filter(|l| l.contains(PORTABLE_MARKER))
        .count();
    assert!(
        marked >= 3,
        "write, ZIP check and installer check: {marked}"
    );
    let write = workflow
        .find(&format!("> \"portable/{PORTABLE_MARKER}\""))
        .unwrap();
    let zip = workflow.find("-windows-x64-portable.zip\" .)").unwrap();
    assert!(write < zip, "the marker is written before the ZIP is made");
}
