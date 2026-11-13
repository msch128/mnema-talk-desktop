# Mnema Talk for Android and iOS (spike)

A spike, not a product: it proves that the desktop host's trust core builds
and runs on the phone. The design, the alternatives, push, calls, the store
requirements and the owner's decisions are in [doc/mobile.md](../doc/mobile.md).

## What is here

- `src/lib.rs` builds the desktop host's own files from
  `experimental/app/src` unchanged: the bundle store and its verification
  (`web_bundle`), the proxy with the cookie jar (`web_proxy`), the TLS stack
  (`web_tls`), the vault (`web_vault`), the WebSocket relay (`web_ws`), the JSON
  limits, the route list, discovery and address parsing. The shared test
  vectors cover them in the desktop crate, where they are tested.
- `src/host.rs`, `src/protocol.rs`, `src/commands.rs`: the phone shell. One
  window with one webview; the host is either in the picker role (no server
  open) or in the role of the one open server. The page's identity script is
  written for the role in force, and every command checks that role.
- `src/android_tls.rs`: the Android certificate check (the system trust
  manager through JNI) and `scripts/android-verifier.gradle.kts`, its Kotlin
  half.
- `page-shim/ipc-bodies.js`: Android's WebView gives the host no request
  bodies, so requests with a body go to the host over IPC (`mobile_api`).
- `src/selftest.rs` (feature `mobile-selftest`, never shipped): the app opens
  the server named at build time on its own and records each step.
- `gen/android`, `gen/apple`: the projects the Tauri CLI generated.
- `.github/workflows/mobile.yml`: the builds and the self-tests.

## Build

CI builds everything (`Mobile spike` workflow): a debug APK for aarch64 and
x86_64, signed with the Android debug key that Gradle generates in that run,
and an unsigned iOS simulator app. Locally you need Rust 1.99.1 with the
Android or iOS targets, the Android SDK and NDK (versions in the workflow) or
Xcode, and the Tauri CLI from `ui/`:

```sh
BUNDLE_DIR=mobile/bundle GH_TOKEN=... bash scripts/stage-web-bundle.sh
cd mobile
node ../ui/node_modules/@tauri-apps/cli/tauri.js android build --debug --apk
node ../ui/node_modules/@tauri-apps/cli/tauri.js ios build --debug --target aarch64-sim --no-sign
```

## State

See the last section of [doc/mobile.md](../doc/mobile.md#spike-what-was-proven).
