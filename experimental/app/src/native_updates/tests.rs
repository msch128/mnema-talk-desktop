use super::*;
use super::{check::*, install::*, logic::*, menu::*, now::*, portable::*, state::*};
use crate::web_bundle::{CHECK_INTERVAL_SECS, Version};
use base64::Engine as _;
use std::path::{Path, PathBuf};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};
use tauri::Manager;
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use url::Url;

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
    assert_eq!(offerable("1.3.3", "1.3.4"), Version::parse("1.3.4"));
    assert_eq!(offerable("1.3.4", "1.3.3"), None);
    assert_eq!(offerable("0.1.9", "0.2.0"), Version::parse("0.2.0"));
    assert_eq!(offerable("0.9.0", "0.10.0"), Version::parse("0.10.0"));
    assert_eq!(offerable("0.3.9", "1.2.0"), Version::parse("1.2.0"));
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
    // The new home of the repository first, the old path (which redirects
    // after the move, and is the only one before it) as the fallback.
    let endpoints: Vec<&str> = updater["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert_eq!(
        endpoints,
        [
            "https://github.com/mnema-xyz/talk-desktop/releases/latest/download/latest.json",
            "https://github.com/msch128/mnema-talk-desktop/releases/latest/download/latest.json",
        ]
    );
    for endpoint in endpoints {
        let url = Url::parse(endpoint).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("github.com"));
        assert!(
            url.path()
                .ends_with("/releases/latest/download/latest.json")
        );
    }
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
    /// The signed portable ZIP of the same version, under its own feed entry.
    portable: Option<Vec<u8>>,
}

fn publish(release: &Release) -> u16 {
    // The server needs its own port in the manifest, so reserve it first.
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let mut platforms = serde_json::json!({ "windows-x86_64": {
        "signature": release.signer.sign(&release.bytes, release.signed_version),
        "url": format!("http://127.0.0.1:{port}/installer.exe"),
    }});
    let mut routes = HashMap::new();
    if let Some(zip) = &release.portable {
        platforms[PORTABLE_TARGET] = serde_json::json!({
            "signature": release.signer.sign(zip, release.signed_version),
            "url": format!("http://127.0.0.1:{port}/portable.zip"),
        });
        routes.insert("/portable.zip".to_owned(), zip.clone());
    }
    let manifest = serde_json::json!({
        "version": release.version,
        "notes": "Test release\nSecond line",
        "pub_date": "2026-10-17T12:00:00Z",
        "platforms": platforms,
    });
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
    app_with_source(trusted, port, dir, false)
}

fn app_with_source(
    trusted: &Keys,
    port: u16,
    dir: &Path,
    github_only: bool,
) -> tauri::App<MockRuntime> {
    app_built(trusted, port, dir, github_only, false)
}

fn app_built(
    trusted: &Keys,
    port: u16,
    dir: &Path,
    github_only: bool,
    portable: bool,
) -> tauri::App<MockRuntime> {
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
        github_only,
        target: Some(
            if portable {
                PORTABLE_TARGET
            } else {
                "windows-x86_64"
            }
            .into(),
        ),
        executable_path: Some(if portable {
            dir.join("app").join(PORTABLE_EXE)
        } else {
            dir.join("Mnema Test.app/Contents/MacOS/mnema")
        }),
        portable,
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
        portable: None,
    }
}

/// Not the product's name: the Windows CI looks for that one in the profile
/// (where the temporary folder is) to tell whether a copy was installed.
const PORTABLE_EXE: &str = "Mnema Portable Test.exe";

/// A ZIP with these files, as the release workflow packs the portable copy.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    out.into_inner()
}

fn portable_zip(program: &[u8]) -> Vec<u8> {
    zip_of(&[
        (PORTABLE_EXE, program),
        (PORTABLE_MARKER, b"portable\n"),
        ("LICENSE", b"new licence"),
        ("licenses/new.txt", b"added"),
    ])
}

// ---------------------------------------------------------- end to end

mod call;
mod feed_compat;
mod flow;
mod package_and_portable;
mod portable_zip;
