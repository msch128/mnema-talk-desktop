use super::*;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};

const CONF: &str = include_str!("../../tauri.conf.json");

// ------------------------------------------------------------ pure logic

#[test]
fn the_interval_matches_the_web_bundles_and_a_clock_set_back_is_due() {
    assert!(check_due(0, 1_000_000));
    assert!(!check_due(1_000_000, 1_000_000 + CHECK_INTERVAL_SECS - 1));
    assert!(check_due(1_000_000, 1_000_000 + CHECK_INTERVAL_SECS));
    assert!(check_due(2_000_000, 1_000_000));
    assert_eq!(CHECK_INTERVAL_SECS, 6 * 60 * 60);
}

#[test]
fn only_strictly_newer_plain_versions_are_offered() {
    assert_eq!(offerable("0.1.0", "0.1.1"), Version::parse("0.1.1"));
    assert_eq!(offerable("0.1.9", "0.2.0"), Version::parse("0.2.0"));
    assert_eq!(offerable("0.9.0", "0.10.0"), Version::parse("0.10.0"));
    for same_or_older in ["0.1.0", "0.0.9", "0.0.99"] {
        assert_eq!(offerable("0.1.0", same_or_older), None);
    }
    for odd in [
        "",
        "v0.2.0",
        "0.2",
        "0.2.0-beta.1",
        "0.2.0+1",
        "01.2.3",
        "0.2.0.1",
        "x",
    ] {
        assert_eq!(offerable("0.1.0", odd), None, "{odd}");
    }
    assert_eq!(offerable("garbage", "0.2.0"), None);
}

#[test]
fn a_skipped_version_stays_quiet_until_a_newer_one_appears() {
    assert!(is_skipped(Some("0.2.0"), "0.2.0"));
    assert!(!is_skipped(Some("0.2.0"), "0.2.1"));
    assert!(!is_skipped(Some("0.2.0"), "0.1.9"));
    assert!(!is_skipped(None, "0.2.0"));
}

#[test]
fn installers_are_fetched_from_github_over_https_only() {
    let ok = |u: &str| download_url_ok(&Url::parse(u).unwrap());
    assert!(ok(
        "https://github.com/msch128/mnema-talk-desktop/releases/download/v0.2.0/a.exe"
    ));
    for bad in [
        "http://github.com/a/b/releases/download/v1/a.exe",
        "https://evil.example/github.com/a.exe",
        "https://github.com.evil.example/a.exe",
        "https://user:pw@github.com/a.exe",
        "https://github.com:8443/a.exe",
        "https://objects.githubusercontent.com/a.exe",
        "file:///a.exe",
    ] {
        assert!(!ok(bad), "{bad}");
    }
}

#[test]
fn the_page_serves_only_its_own_files_and_navigates_nowhere_else() {
    for (path, kind) in [
        ("/__update/", "text/html"),
        ("/__update/index.html", "text/html"),
        ("/__update/update.css", "text/css"),
        ("/__update/update.js", "text/javascript"),
    ] {
        assert!(
            asset(path).is_some_and(|(k, body)| k.starts_with(kind) && !body.is_empty()),
            "{path}"
        );
    }
    for path in [
        "/",
        "/index.html",
        "/__update/../index.html",
        "/__update/secret",
        "/api/x",
    ] {
        assert!(asset(path).is_none(), "{path}");
    }
    let app = crate::web_client::app_url();
    assert!(ui_url(&app.join("__update/index.html").unwrap()));
    assert!(!ui_url(&app));
    assert!(!ui_url(
        &Url::parse("https://example.com/__update/index.html").unwrap()
    ));
    // No inline script or remote origin may run in the update window.
    assert!(UI_CSP.contains("default-src 'none'") && !UI_CSP.contains("unsafe-inline"));
    assert!(!INDEX.contains("<script>") && !INDEX.contains("onclick"));
}

#[test]
fn the_pinned_key_and_endpoint_in_the_config_are_usable() {
    let conf: serde_json::Value = serde_json::from_str(CONF).unwrap();
    let updater = &conf["plugins"]["updater"];
    let key = base64::engine::general_purpose::STANDARD
        .decode(updater["pubkey"].as_str().unwrap())
        .unwrap();
    assert!(minisign_verify::PublicKey::decode(std::str::from_utf8(&key).unwrap()).is_ok());
    let endpoints = updater["endpoints"].as_array().unwrap();
    assert_eq!(endpoints.len(), 1);
    let url = Url::parse(endpoints[0].as_str().unwrap()).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("github.com"));
    assert!(
        url.path()
            .ends_with("/releases/latest/download/latest.json")
    );
    assert_eq!(updater["requireSignedVersion"], true);
    assert!(updater.get("dangerousInsecureTransportProtocol").is_none());
    assert!(updater.get("allowDowngrades").is_none());
    // Per-user install: the passive update needs no administrator prompt.
    assert_eq!(
        conf["bundle"]["windows"]["nsis"]["installMode"],
        "currentUser"
    );
    assert_eq!(updater["windows"]["installMode"], "passive");
    // Config and crate agree on the version the updater compares.
    assert_eq!(conf["version"], env!("CARGO_PKG_VERSION"));
}

// ------------------------------------------------------------ signing

struct Keys {
    secret: minisign::SecretKey,
    public: minisign::PublicKey,
}

fn keys() -> Keys {
    let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    Keys {
        secret: pair.sk,
        public: pair.pk,
    }
}

fn b64(text: String) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

impl Keys {
    fn pubkey(&self) -> String {
        b64(self.public.to_box().unwrap().to_string())
    }

    /// The signature the Tauri CLI would write, as the `.sig` file text in base64.
    fn sign(&self, bytes: &[u8], signed_version: &str) -> String {
        let comment = format!("timestamp:1\tfile:Test.exe\tversion:{signed_version}");
        let sig = minisign::sign(
            Some(&self.public),
            &self.secret,
            bytes,
            Some(&comment),
            None,
        )
        .unwrap();
        b64(sig.to_string())
    }
}

#[test]
fn a_package_verifies_only_with_the_right_key_bytes_and_signed_version() {
    let (k, other) = (keys(), keys());
    let bytes = b"installer".as_slice();
    let sig = k.sign(bytes, "0.2.0");
    assert_eq!(verify_package(bytes, &sig, &k.pubkey(), "0.2.0"), Ok(()));
    assert_eq!(
        verify_package(b"installer!", &sig, &k.pubkey(), "0.2.0"),
        Err(VerifyError::Mismatch)
    );
    assert_eq!(
        verify_package(bytes, &sig, &other.pubkey(), "0.2.0"),
        Err(VerifyError::Mismatch)
    );
    assert_eq!(
        verify_package(bytes, &sig, &k.pubkey(), "0.3.0"),
        Err(VerifyError::Version)
    );
    assert_eq!(
        verify_package(bytes, &sig, "!!", "0.2.0"),
        Err(VerifyError::Key)
    );
    assert_eq!(
        verify_package(bytes, "!!", &k.pubkey(), "0.2.0"),
        Err(VerifyError::Signature)
    );
    // A signature that records no version at all is refused.
    let bare =
        minisign::sign(Some(&k.public), &k.secret, bytes, Some("timestamp:1"), None).unwrap();
    assert_eq!(
        verify_package(bytes, &b64(bare.to_string()), &k.pubkey(), "0.2.0"),
        Err(VerifyError::Version)
    );
}

// ---------------------------------------------------- local release server

struct Release<'a> {
    version: &'a str,
    bytes: Vec<u8>,
    /// What the signature claims to be signed for.
    signed_version: &'a str,
    signer: &'a Keys,
    /// What the server hands out (differs from `bytes` when tampered).
    served: Option<Vec<u8>>,
}

fn publish(release: &Release) -> u16 {
    // The server needs its own port in the manifest, so reserve it first.
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let manifest = serde_json::json!({
        "version": release.version,
        "notes": "Test release\nSecond line",
        "pub_date": "2026-10-17T12:00:00Z",
        "platforms": { "windows-x86_64": {
            "signature": release.signer.sign(&release.bytes, release.signed_version),
            "url": format!("http://127.0.0.1:{port}/installer.exe"),
        }}
    });
    let mut routes = HashMap::new();
    routes.insert("/latest.json".to_owned(), manifest.to_string().into_bytes());
    routes.insert(
        "/installer.exe".to_owned(),
        release
            .served
            .clone()
            .unwrap_or_else(|| release.bytes.clone()),
    );
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    serve_on(listener, routes);
    port
}

/// Serves fixed paths over plain HTTP on loopback for the rest of the process.
fn serve_on(listener: TcpListener, routes: HashMap<String, Vec<u8>>) {
    let routes = Arc::new(routes);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, body) = match routes.get(&path) {
                Some(body) => ("200 OK", body.clone()),
                None => ("404 Not Found", Vec::new()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
}

fn data_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mnema-native-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A running (mock) app at version 0.1.0 that trusts `trusted` and looks for
/// releases on `port`.
fn app_with(trusted: &Keys, port: u16, dir: &Path) -> tauri::App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        serde_json::json!({
            "pubkey": trusted.pubkey(),
            "endpoints": [format!("http://127.0.0.1:{port}/latest.json")],
            "dangerousInsecureTransportProtocol": true,
            "requireSignedVersion": true,
        }),
    );
    let app = mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    let source = Source {
        endpoints: vec![Url::parse(&format!("http://127.0.0.1:{port}/latest.json")).unwrap()],
        pubkey: trusted.pubkey(),
        github_only: false,
        target: Some("windows-x86_64".into()),
        executable_path: Some(dir.join("Mnema Test.app/Contents/MacOS/mnema")),
    };
    let current = app.package_info().version.to_string();
    assert_eq!(current, "0.1.0", "the mock app is the old version");
    app.manage(NativeUpdates::new(current, Some(dir.to_path_buf()), source));
    app
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

fn release<'a>(version: &'a str, signer: &'a Keys) -> Release<'a> {
    Release {
        version,
        bytes: b"MZ pretend installer of the new version".to_vec(),
        signed_version: version,
        signer,
        served: None,
    }
}

// ---------------------------------------------------------- end to end

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
    let (update, bytes) = prepare_pending(&handle).expect("verified installer for the next start");
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
    assert!(prepare_pending(&app.handle().clone()).is_none());
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
    assert!(prepare_pending(&app.handle().clone()).is_none());
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
    assert!(prepare_pending(app.handle()).is_none());
}
