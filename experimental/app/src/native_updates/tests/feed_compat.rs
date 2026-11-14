//! The feed with the portable entry, read the way installed clients up to
//! 1.3.5 read it: they set no updater target, so the plugin looks for
//! `{os}-{arch}-{installer}`, then `{os}-{arch}`. The new key is none of those,
//! and the entries they read stay as they were.
use super::*;

const OLD_KEYS: [&str; 6] = [
    "windows-x86_64",
    "darwin-aarch64",
    "linux-x86_64",
    "linux-aarch64",
    "linux-x86_64-deb",
    "linux-aarch64-deb",
];

/// The shape `scripts/update-manifest.mjs` writes, with one URL per entry.
fn full_feed(keys: &Keys, port: u16, version: &str, bytes: &[u8]) -> serde_json::Value {
    let mut platforms = serde_json::Map::new();
    for key in OLD_KEYS.iter().chain([&PORTABLE_TARGET]) {
        platforms.insert(
            (*key).to_owned(),
            serde_json::json!({
                "signature": keys.sign(bytes, version),
                "url": format!("http://127.0.0.1:{port}/{key}"),
            }),
        );
    }
    serde_json::json!({
        "version": version,
        "notes": "n",
        "pub_date": "2026-11-13T12:00:00Z",
        "platforms": platforms,
    })
}

#[test]
fn every_old_entry_is_read_as_before_and_the_portable_key_is_a_new_one() {
    let feed = full_feed(&keys(), 1, "1.3.6", b"x");
    let release: tauri_plugin_updater::RemoteRelease = serde_json::from_value(feed).unwrap();
    for key in OLD_KEYS {
        assert_eq!(
            release.download_url(key).unwrap().path(),
            format!("/{key}"),
            "{key}"
        );
    }
    // No installer name of the plugin (nsis, msi, app, appimage, deb, rpm)
    // makes `windows-x86_64-<installer>` the portable key.
    for installer in ["nsis", "msi", "app", "appimage", "deb", "rpm"] {
        assert_ne!(format!("windows-x86_64-{installer}"), PORTABLE_TARGET);
    }
}

#[test]
fn an_installed_client_without_a_target_still_picks_its_own_entry() {
    let own = if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x86_64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-aarch64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else {
        return;
    };
    let keys = keys();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut routes = HashMap::new();
    routes.insert(
        "/latest.json".to_owned(),
        full_feed(&keys, port, "0.2.0", b"x")
            .to_string()
            .into_bytes(),
    );
    serve_on(listener, routes);
    let dir = data_dir("feed-compat");
    let app = app_with(&keys, port, &dir);
    // What an installed 1.3.5 runs with: no target of its own.
    let source = Source {
        target: None,
        ..app.state::<NativeUpdates>().source.clone()
    };
    let found = run(fetch(app.handle(), &source))
        .unwrap()
        .expect("an update");
    assert_eq!(found.version, "0.2.0");
    assert_eq!(found.download_url.path(), format!("/{own}"));
    // The portable copy of this version reads its own key.
    let portable = Source {
        target: Some(PORTABLE_TARGET.into()),
        ..source
    };
    let found = run(fetch(app.handle(), &portable))
        .unwrap()
        .expect("an update");
    assert_eq!(found.download_url.path(), format!("/{PORTABLE_TARGET}"));
}
