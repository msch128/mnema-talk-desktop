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

fn main() {
    embed_builtin_bundle();
    #[cfg(feature = "shell")]
    {
        #[cfg(not(any(feature = "synthetic-media-fixture", feature = "native-crypto")))]
        const COMMANDS: &[&str] = &[
            "native_context",
            "desktop_open_instance",
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
            "rail_move",
            "rail_remove",
            "rail_reconnect",
            "desktop_gaming_sync",
            "desktop_gaming_snapshot",
            "desktop_gaming_control",
            "desktop_gaming_settings",
            "native_update_state",
            "native_update_action",
            "native_connect",
            "native_metadata_request",
            "native_personal_metadata_request",
            "native_public_metadata_request",
            "native_auth_login",
            "native_auth_password",
            "native_auth_register",
            "native_admin_request",
            "native_auth_me",
            "native_auth_refresh",
            "native_auth_logout",
            "native_disconnect",
            "native_request_cancel",
            "native_socket_open",
            "native_socket_send",
            "native_socket_close",
        ];
        #[cfg(feature = "native-crypto")]
        const COMMANDS: &[&str] = &[
            "native_context",
            "desktop_open_instance",
            "desktop_gaming_sync",
            "desktop_gaming_snapshot",
            "desktop_gaming_control",
            "desktop_gaming_settings",
            "native_update_state",
            "native_update_action",
            "native_connect",
            "native_metadata_request",
            "native_personal_metadata_request",
            "native_public_metadata_request",
            "native_auth_login",
            "native_auth_password",
            "native_auth_register",
            "native_admin_request",
            "native_auth_me",
            "native_auth_refresh",
            "native_auth_logout",
            "native_disconnect",
            "native_request_cancel",
            "native_socket_open",
            "native_socket_send",
            "native_socket_close",
            "native_trust_begin_first_root",
            "native_trust_request_confirmation",
            "native_trust_cancel",
            "native_trust_read_status",
            "native_chat_publish",
            "native_chat_receive",
            "native_chat_mutate",
            "native_chat_snapshot",
            "native_device_remove",
        ];
        #[cfg(feature = "synthetic-media-fixture")]
        const COMMANDS: &[&str] = &[
            "native_context",
            "desktop_open_instance",
            "desktop_gaming_sync",
            "desktop_gaming_snapshot",
            "desktop_gaming_control",
            "desktop_gaming_settings",
            "native_update_state",
            "native_update_action",
            "native_connect",
            "native_metadata_request",
            "native_personal_metadata_request",
            "native_public_metadata_request",
            "native_auth_login",
            "native_auth_password",
            "native_auth_register",
            "native_admin_request",
            "native_auth_me",
            "native_auth_refresh",
            "native_auth_logout",
            "native_disconnect",
            "native_request_cancel",
            "native_socket_open",
            "native_socket_send",
            "native_socket_close",
            "native_fixture_media_open",
            "native_fixture_media_send",
            "native_fixture_media_close",
        ];
        tauri_build::try_build(
            tauri_build::Attributes::new()
                .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
        )
        .expect("native preview capabilities could not be validated");
    }
}
