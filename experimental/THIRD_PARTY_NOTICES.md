# Desktop host third-party notices

This development build uses the pinned dependencies in app/Cargo.lock and the canonical ../mnema-talk/web/package-lock.json. The full original package license and notice texts are in licenses/texts/; licenses/manifest.json maps package/version/checksum to those texts. The graph includes optional and build dependencies conservatively and does not claim measured linking. The complete Rust 1.99.1 standard-library copyright notice, Inter OFL attribution, and build CSS notices are retained in licenses/.

`keyring` 3.6.3 (MIT OR Apache-2.0) stores the session cookie in the macOS Keychain or Windows Credential Manager, with its macOS and Windows platform dependencies; they are in the locked inventory and licenses/manifest.json.

`tauri-plugin-updater` 2.14.0 (Apache-2.0 OR MIT) and `minisign-verify` 0.3.0 (MIT) update the installed client: the installer is run only after its minisign signature verifies against the public key built into the app. The plugin's archive dependencies (`tar`, `zip`, `flate2`) are in the locked inventory and licenses/manifest.json. The private updater key is not in this repository. No code-signing certificate is configured; the installer is not Authenticode-signed.

The copied existing web DeepFilterNet WASM/model has an additional provenance and transitive-license qualification gap. Its existing notices are preserved; this checkpoint makes no claim that a final binary distribution license closure is complete.

See the project root THIRD_PARTY_NOTICES.md for existing web/backend dependencies and LICENSE for project terms.

