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
    let exe = dir.join("Mnema Desktop.exe");
    assert!(!portable_at(&exe), "an installed copy has no marker");
    std::fs::write(dir.join(PORTABLE_MARKER), b"portable\n").unwrap();
    assert!(portable_at(&exe));
    // Elsewhere, or as a directory, the name means nothing.
    assert!(!portable_at(&dir.join("sub/Mnema Desktop.exe")));
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

/// The folder of a portable copy before the update: the program, its marker,
/// a notice and the data folder with the servers and sign-ins.
fn portable_folder(dir: &Path) -> PathBuf {
    let app = dir.join("app");
    std::fs::create_dir_all(app.join("mnema-data/data")).unwrap();
    std::fs::write(app.join(PORTABLE_EXE), b"old program").unwrap();
    std::fs::write(app.join(PORTABLE_MARKER), b"portable\n").unwrap();
    std::fs::write(app.join("LICENSE"), b"old licence").unwrap();
    std::fs::write(app.join("mnema-data/data/servers.json"), b"my servers").unwrap();
    app
}

#[test]
fn a_portable_copy_downloads_its_signed_zip_replaces_its_program_and_keeps_its_data() {
    let keys = keys();
    let dir = data_dir("portable");
    let folder = portable_folder(&dir);
    let mut rel = release("0.2.0", &keys);
    rel.portable = Some(portable_zip(b"new program"));
    let port = publish(&rel);
    // A leftover installer from before: it must neither be offered nor run.
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
    let restarted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = restarted.clone();
    *state.restart.lock().unwrap() = Some(Arc::new(move || {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }));
    let snap = state.snapshot();
    assert_eq!((snap.phase, snap.portable), (Phase::Idle, true));
    assert!(!dir.join(PENDING_FILE).exists());
    assert!(run(prepare_pending(&handle)).is_none());
    run(apply_pending(&handle));

    assert!(run(look(&handle, false)), "the new version is announced");
    let snap = state.snapshot();
    assert_eq!(
        (snap.phase, snap.version.as_deref()),
        (Phase::Available, Some("0.2.0"))
    );
    assert!(menu_view(&snap, false).now_enabled);

    // "On next start" is not for a portable copy: nothing is kept.
    run(install(&handle, Mode::Later));
    assert_eq!(state.snapshot().phase, Phase::Available);
    assert!(!dir.join(PENDING_FILE).exists());

    run(install(&handle, Mode::Now));
    assert!(
        restarted.load(std::sync::atomic::Ordering::SeqCst),
        "{:?}",
        state.snapshot()
    );
    assert_replaced(&folder);
}

/// The new files are in place, the data folder is untouched, and the next
/// start deletes the program that was replaced.
fn assert_replaced(folder: &Path) {
    let read = |name: &str| std::fs::read(folder.join(name)).unwrap();
    assert_eq!(read(PORTABLE_EXE), b"new program");
    assert_eq!(read(&format!("{PORTABLE_EXE}{OLD_SUFFIX}")), b"old program");
    assert_eq!(read("LICENSE"), b"new licence");
    assert_eq!(read("licenses/new.txt"), b"added");
    assert_eq!(read(PORTABLE_MARKER), b"portable\n");
    assert_eq!(read("mnema-data/data/servers.json"), b"my servers");
    assert!(!folder.join(STAGE).exists());
    // The next start deletes what was replaced.
    assert!(clean_up(&folder.join(PORTABLE_EXE)));
    assert!(!folder.join(format!("{PORTABLE_EXE}{OLD_SUFFIX}")).exists());
    assert_eq!(read(PORTABLE_EXE), b"new program");
}

#[test]
fn an_installed_copy_takes_the_installer_and_never_the_portable_zip() {
    let keys = keys();
    let dir = data_dir("installed-not-zip");
    let mut rel = release("0.2.0", &keys);
    rel.portable = Some(portable_zip(b"new program"));
    let port = publish(&rel);
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    assert_eq!(std::fs::read(dir.join(PENDING_FILE)).unwrap(), rel.bytes);
}

#[test]
fn a_tampered_portable_zip_is_refused_and_nothing_changes() {
    let keys = keys();
    let dir = data_dir("portable-tampered");
    let folder = portable_folder(&dir);
    // The feed carries the signature of one ZIP, the server hands out another.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let feed = serde_json::json!({
        "version": "0.2.0",
        "platforms": { PORTABLE_TARGET: {
            "signature": keys.sign(&portable_zip(b"new program"), "0.2.0"),
            "url": format!("http://127.0.0.1:{port}/portable.zip"),
        }}
    });
    let mut routes = HashMap::new();
    routes.insert("/latest.json".to_owned(), feed.to_string().into_bytes());
    routes.insert("/portable.zip".to_owned(), portable_zip(b"evil program"));
    serve_on(listener, routes);
    let app = app_built(&keys, port, &dir, false, true);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Now));
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Verify))
    );
    assert_eq!(
        std::fs::read(folder.join(PORTABLE_EXE)).unwrap(),
        b"old program"
    );
    assert!(!folder.join(STAGE).exists());
}

#[test]
fn the_menu_offers_update_now_only_when_a_version_waits_in_both_languages() {
    let snap = |phase, version: Option<&str>| Snapshot {
        current: "1.3.5".into(),
        phase,
        version: version.map(str::to_owned),
        notes: None,
        received: 0,
        total: None,
        mode: None,
        error: None,
        portable: false,
        confirm_call: false,
        skipped: false,
    };
    let idle = menu_view(&snap(Phase::Idle, None), false);
    assert_eq!(
        (
            idle.title,
            idle.now_enabled,
            idle.check,
            idle.version.as_str()
        ),
        (
            "Updates",
            false,
            "Check for updates…",
            "Mnema Desktop 1.3.5"
        )
    );
    for phase in [
        Phase::Available,
        Phase::Ready,
        Phase::Waiting,
        Phase::Failed,
    ] {
        let view = menu_view(&snap(phase, Some("1.3.6")), false);
        assert_eq!(
            (view.title, view.now_enabled),
            ("Update available •", true),
            "{phase:?}"
        );
    }
    // A failed check names no version: there is nothing to install.
    assert!(!menu_view(&snap(Phase::Failed, None), false).now_enabled);
    for phase in [
        Phase::Checking,
        Phase::Downloading,
        Phase::Installing,
        Phase::UpToDate,
    ] {
        assert!(
            !menu_view(&snap(phase, Some("1.3.6")), false).now_enabled,
            "{phase:?}"
        );
    }
    let german = menu_view(&snap(Phase::Available, Some("1.3.6")), true);
    assert_eq!(
        (german.title, german.now, german.check),
        (
            "Update verfügbar •",
            "Jetzt aktualisieren",
            "Nach Updates suchen …"
        )
    );
}

/// The release workflow is what puts the marker into the portable ZIP, signs
/// the ZIP for the updater and keeps the marker out of the installer; this ties
/// its spelling to the one the program reads.
#[test]
fn the_release_workflow_marks_and_signs_the_portable_zip_and_checks_the_installer_has_none() {
    let workflow = include_str!("../../../../../.github/workflows/desktop-release.yml");
    let marked = workflow
        .lines()
        .filter(|l| l.contains(PORTABLE_MARKER))
        .count();
    assert!(
        marked >= 4,
        "write, two ZIP checks and the installer check: {marked}"
    );
    let write = workflow
        .find(&format!("'{PORTABLE_MARKER}'), \"portable`n\")"))
        .unwrap();
    let pack = workflow.find("& tar.exe -a -c -f $zip").unwrap();
    let sign = workflow
        .find("signer sign --app-version $version $zip")
        .unwrap();
    assert!(
        write < pack && pack < sign,
        "marked, then packed, then signed"
    );
    // The feed names the ZIP under the key the portable copy reads.
    assert!(workflow.contains(&format!("windows-x86_64 {PORTABLE_TARGET} ")));
    assert!(workflow.contains("--portable \"$portable\" --portable-sig \"$portable.sig\""));
}
