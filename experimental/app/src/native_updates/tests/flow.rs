use super::*;
#[test]
fn a_signed_newer_release_is_found_downloaded_verified_and_kept_for_the_next_start() {
    let keys = keys();
    let dir = data_dir("later");
    let rel = release("0.2.0", &keys);
    let port = publish(&rel);
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();

    assert_eq!(state.snapshot().phase, Phase::Idle);
    assert!(run(look(&handle, false)), "a new version is announced");
    let snap = state.snapshot();
    assert_eq!(
        (snap.phase, snap.version.as_deref()),
        (Phase::Available, Some("0.2.0"))
    );
    assert_eq!(snap.notes.as_deref(), Some("Test release\nSecond line"));

    run(install(&handle, Mode::Later));
    let snap = state.snapshot();
    assert_eq!(snap.phase, Phase::Ready, "{snap:?}");
    assert_eq!(snap.received, rel.bytes.len() as u64);
    assert_eq!(std::fs::read(dir.join(PENDING_FILE)).unwrap(), rel.bytes);

    // The app is closed and started again: the installer is still verified
    // against the manifest before it would run.
    drop(app);
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    assert_eq!(
        handle.state::<NativeUpdates>().snapshot().phase,
        Phase::Ready
    );
    // Without a trust statement fetched in this run the installer is kept,
    // not discarded; once one is fresh it is re-verified and applied.
    let trusted = crate::web_client::Trusted::new(None, None);
    handle.manage(trusted);
    assert!(run(prepare_pending(&handle)).is_none());
    assert!(
        dir.join(PENDING_FILE).exists(),
        "kept for the fetch to succeed"
    );
    handle.state::<crate::web_client::Trusted>().set_fresh(true);
    let (update, bytes) =
        run(prepare_pending(&handle)).expect("verified installer for the next start");
    assert_eq!((update.version.as_str(), bytes), ("0.2.0", rel.bytes));
}

#[test]
fn a_tampered_download_is_refused_and_nothing_is_kept() {
    let keys = keys();
    let dir = data_dir("tampered");
    let mut rel = release("0.2.0", &keys);
    let mut evil = rel.bytes.clone();
    evil[3] ^= 1;
    rel.served = Some(evil);
    let port = publish(&rel);
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();

    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    let snap = state.snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Verify))
    );
    assert!(!dir.join(PENDING_FILE).exists());
    assert_eq!(Persisted::load(Some(&dir)).pending, None);
}

#[test]
fn a_release_signed_by_another_key_is_refused() {
    let (ours, attacker) = (keys(), keys());
    let dir = data_dir("otherkey");
    let port = publish(&release("0.2.0", &attacker));
    let app = app_with(&ours, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Verify))
    );
}

#[test]
fn an_older_installer_cannot_pass_as_a_newer_version() {
    let keys = keys();
    let dir = data_dir("mixup");
    // Validly signed, but for 0.1.5; the manifest claims 0.9.0.
    let mut rel = release("0.9.0", &keys);
    rel.signed_version = "0.1.5";
    let port = publish(&rel);
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Verify))
    );
    assert!(!dir.join(PENDING_FILE).exists());
}

#[test]
fn the_same_or_an_older_release_is_never_offered() {
    let keys = keys();
    for (name, version) in [("same", "0.1.0"), ("older", "0.0.9")] {
        let dir = data_dir(name);
        let port = publish(&release(version, &keys));
        let app = app_with(&keys, port, &dir);
        let handle = app.handle().clone();
        assert!(!run(look(&handle, true)), "{version}");
        let snap = handle.state::<NativeUpdates>().snapshot();
        assert_eq!(
            (snap.phase, snap.version),
            (Phase::UpToDate, None),
            "{version}"
        );
        // Nothing to install: asking for it changes nothing and keeps nothing.
        run(install(&handle, Mode::Later));
        assert!(!matches!(
            handle.state::<NativeUpdates>().snapshot().phase,
            Phase::Ready | Phase::Downloading
        ));
        assert!(!dir.join(PENDING_FILE).exists());
    }
}

#[test]
fn an_unreachable_server_is_silent_in_the_background_and_reported_on_request() {
    let keys = keys();
    let dir = data_dir("offline");
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let app = app_with(&keys, dead, &dir);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();
    assert!(!run(look(&handle, false)));
    assert_eq!(state.snapshot().phase, Phase::Idle);
    assert!(!run(look(&handle, true)));
    let snap = state.snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Offline))
    );
}

#[test]
fn a_rejected_download_url_is_a_failure_and_keeps_the_installer() {
    let keys = keys();
    let dir = data_dir("bad-url");
    let port = publish(&release("0.2.0", &keys));
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    assert_eq!(
        handle.state::<NativeUpdates>().snapshot().phase,
        Phase::Ready
    );
    drop(app);

    // The manifest now points outside GitHub (the local test server).
    let app = app_with_source(&keys, port, &dir, true);
    let handle = app.handle().clone();
    assert!(!run(look(&handle, true)));
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(
        (snap.phase, snap.error),
        (Phase::Failed, Some(Failure::Verify))
    );
    assert!(run(prepare_pending(&handle)).is_none());
    assert!(
        dir.join(PENDING_FILE).exists(),
        "a rejected URL is not a newer release; the installer stays"
    );
}

#[test]
fn a_failed_check_does_not_count_as_a_completed_one() {
    let keys = keys();
    let dir = data_dir("failed-check");
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let app = app_with(&keys, dead, &dir);
    let handle = app.handle().clone();
    run(look(&handle, false));
    assert_eq!(Persisted::load(Some(&dir)).last_check, 0);
    let port = publish(&release("0.1.0", &keys));
    let app = app_with(&keys, port, &dir);
    run(look(&app.handle().clone(), false));
    assert_ne!(Persisted::load(Some(&dir)).last_check, 0);
}

#[test]
fn a_menu_text_is_only_set_when_it_changes() {
    let dir = data_dir("menu");
    let keys = keys();
    let app = app_with(&keys, 1, &dir);
    let handle = app.handle().clone();
    let state = handle.state::<NativeUpdates>();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    state.menu.lock().unwrap().items.push(Arc::new(move |_| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    for _ in 0..5 {
        state.with(|i| i.snapshot.received += 1);
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    state.with(|i| i.snapshot.phase = Phase::Failed);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn an_oversized_installer_is_abandoned() {
    assert!(!over_cap(0, None));
    assert!(!over_cap(MAX_INSTALLER, Some(MAX_INSTALLER)));
    assert!(over_cap(0, Some(MAX_INSTALLER + 1)));
    assert!(over_cap(MAX_INSTALLER + 1, None));
}

#[test]
fn skipping_silences_one_version_and_survives_a_restart() {
    let keys = keys();
    let dir = data_dir("skip");
    let port = publish(&release("0.2.0", &keys));
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    assert!(run(look(&handle, false)));
    skip(&handle);
    assert_eq!(
        handle.state::<NativeUpdates>().snapshot().phase,
        Phase::Idle
    );
    assert_eq!(
        Persisted::load(Some(&dir)).skipped.as_deref(),
        Some("0.2.0")
    );
    drop(app);

    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    assert!(
        !run(look(&handle, false)),
        "a skipped version is not announced"
    );
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(
        (snap.phase, snap.version.as_deref()),
        (Phase::Available, Some("0.2.0"))
    );

    // A newer release is announced again.
    let newer = publish(&release("0.2.1", &keys));
    let app = app_with(&keys, newer, &dir);
    assert!(run(look(&app.handle().clone(), false)));
}

#[test]
fn a_kept_installer_that_changed_on_disk_or_went_stale_is_dropped() {
    let keys = keys();
    let dir = data_dir("stale");
    let port = publish(&release("0.2.0", &keys));
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    assert_eq!(
        handle.state::<NativeUpdates>().snapshot().phase,
        Phase::Ready
    );
    drop(app);

    // Changed on disk between the two starts.
    let mut bytes = std::fs::read(dir.join(PENDING_FILE)).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(dir.join(PENDING_FILE), &bytes).unwrap();
    let app = app_with(&keys, port, &dir);
    assert!(run(prepare_pending(&app.handle().clone())).is_none());
    assert!(!dir.join(PENDING_FILE).exists());
    assert_eq!(Persisted::load(Some(&dir)).pending, None);
    drop(app);

    // A newer release replaced the one that was kept.
    let dir = data_dir("stale2");
    let port = publish(&release("0.2.0", &keys));
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    drop(app);
    let newer = publish(&release("0.3.0", &keys));
    let app = app_with(&keys, newer, &dir);
    assert!(run(prepare_pending(&app.handle().clone())).is_none());
    assert!(!dir.join(PENDING_FILE).exists());
}

#[test]
fn a_kept_installer_for_a_version_that_is_already_installed_is_forgotten_at_start() {
    let keys = keys();
    let dir = data_dir("installed");
    Persisted {
        pending: Some(Pending {
            version: "0.1.0".into(),
            signature: "x".into(),
        }),
        ..Persisted::default()
    }
    .save(Some(&dir));
    std::fs::write(dir.join(PENDING_FILE), b"old").unwrap();
    let app = app_with(&keys, 1, &dir);
    assert_eq!(
        app.handle().state::<NativeUpdates>().snapshot().phase,
        Phase::Idle
    );
    assert!(!dir.join(PENDING_FILE).exists());
}

#[test]
fn a_damaged_state_file_starts_fresh() {
    let dir = data_dir("damaged");
    std::fs::write(dir.join(STATE_FILE), b"{not json").unwrap();
    assert_eq!(Persisted::load(Some(&dir)), Persisted::default());
}

#[test]
fn nothing_new_is_installed_without_a_fresh_trust_statement() {
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    assert!(
        installs_allowed(app.handle()),
        "no web client state: unrestricted"
    );
    // A client that has no fresh statement (none yet, expired, or the fetch
    // failed this round) installs nothing new.
    app.manage(crate::web_client::Trusted::new(None, None));
    assert!(!installs_allowed(app.handle()));
    assert!(run(prepare_pending(app.handle())).is_none());
}

#[test]
fn an_install_without_a_fresh_trust_statement_blames_the_trust_not_the_signature() {
    let keys = keys();
    let dir = data_dir("untrusted");
    let port = publish(&release("0.2.0", &keys));
    let app = app_with(&keys, port, &dir);
    let handle = app.handle().clone();
    // No trust statement was fetched in this run: nothing new is installed.
    handle.manage(crate::web_client::Trusted::new(None, None));
    run(look(&handle, true));
    run(install(&handle, Mode::Later));
    let snap = handle.state::<NativeUpdates>().snapshot();
    assert_eq!(snap.phase, Phase::Failed, "{snap:?}");
    // The update page words each failure by this name; "verify" would tell the
    // person the signature is wrong, when it is the trust list that is missing.
    assert_eq!(serde_json::to_value(snap.error).unwrap(), "untrusted");
    assert_eq!(snap.received, 0, "nothing was downloaded");
    assert!(!dir.join(PENDING_FILE).exists());
}

// -------------------------------------------------------- shared vectors
