/// The signed web app bundle compiled into the executable: `MNEMA_BUNDLE_ZIP`,
/// else `bundle/builtin.zip`. A build without one still compiles (the client
/// then has to download a bundle), unless `MNEMA_REQUIRE_BUNDLE` is set, which
/// release builds do.
fn embed_builtin_bundle() {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=MNEMA_BUNDLE_ZIP");
    println!("cargo:rerun-if-env-changed=MNEMA_REQUIRE_BUNDLE");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("builtin-bundle.zip");
    let explicit = env::var_os("MNEMA_BUNDLE_ZIP").map(PathBuf::from);
    let source = explicit
        .clone()
        .unwrap_or_else(|| PathBuf::from("bundle/builtin.zip"));
    println!("cargo:rerun-if-changed={}", source.display());
    match fs::copy(&source, &out) {
        Ok(_) => {}
        Err(_) => {
            let required =
                env::var_os("MNEMA_REQUIRE_BUNDLE").is_some_and(|v| !v.is_empty() && v != "0");
            assert!(
                explicit.is_none() && !required,
                "the signed web bundle {} is required but missing",
                source.display()
            );
            fs::write(&out, b"").expect("write the empty bundle placeholder");
            println!(
                "cargo:warning=no built-in web bundle ({}); the client must download one",
                source.display()
            );
        }
    }
}

/// Commands the shell registers: the pages of servers, the rail, the update
/// bridge, the gaming controls and the notification bridge.
#[cfg(feature = "shell")]
const COMMANDS: &[&str] = &[
    "desktop_open_instance",
    "desktop_gaming_sync",
    "desktop_gaming_snapshot",
    "desktop_gaming_control",
    "desktop_gaming_settings",
    "desktop_gaming_candidate",
    "desktop_gaming_add_game",
    "desktop_gaming_remove_game",
    "native_update_state",
    "native_update_action",
    "desktop_ws_open",
    "desktop_ws_send",
    "desktop_ws_close",
    "desktop_notify_open",
    "desktop_notify",
    "desktop_notify_permission",
    "desktop_notify_request_permission",
    "desktop_set_badge",
    "desktop_report_unread",
    "rail_state",
    "rail_activate",
    "rail_add",
    "rail_cancel_add",
    "rail_edit",
    "rail_menu",
    "rail_dialog",
    "dialog_done",
    "rail_icon",
    "rail_remove",
    "rail_reconnect",
    "rail_notify_get",
    "rail_notify_set",
];

fn main() {
    embed_builtin_bundle();
    #[cfg(feature = "shell")]
    {
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
        )
        .expect("the capabilities could not be validated");
    }
}
