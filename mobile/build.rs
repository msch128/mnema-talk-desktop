/// The signed web app bundle compiled into the app: `MNEMA_BUNDLE_ZIP`, else
/// `bundle/builtin.zip` (staged by `scripts/stage-web-bundle.sh` with
/// `BUNDLE_DIR=mobile/bundle`). The app has no other way to get one yet, so a
/// build without it fails unless `MNEMA_ALLOW_NO_BUNDLE=1`, which only a
/// compile check may set.
fn embed_builtin_bundle() {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=MNEMA_BUNDLE_ZIP");
    println!("cargo:rerun-if-env-changed=MNEMA_ALLOW_NO_BUNDLE");
    println!("cargo:rerun-if-env-changed=MNEMA_SELFTEST_SERVER");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("builtin-bundle.zip");
    let source = env::var_os("MNEMA_BUNDLE_ZIP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bundle/builtin.zip"));
    println!("cargo:rerun-if-changed={}", source.display());
    if fs::copy(&source, &out).is_err() {
        let allowed = env::var_os("MNEMA_ALLOW_NO_BUNDLE").is_some_and(|v| v == "1");
        assert!(
            allowed,
            "the signed web bundle {} is required but missing",
            source.display()
        );
        fs::write(&out, b"").expect("write the empty bundle placeholder");
        println!(
            "cargo:warning=no built-in web bundle ({}); the app cannot show anything",
            source.display()
        );
    }
}

/// Commands of the one page. The capability file allows them to the main
/// webview; each command checks the role the host is in.
const COMMANDS: &[&str] = &[
    "desktop_open_instance",
    "desktop_ws_open",
    "desktop_ws_send",
    "desktop_ws_close",
    "mobile_api",
];

fn main() {
    embed_builtin_bundle();
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("the capabilities could not be validated");
}
