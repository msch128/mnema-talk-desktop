# Security review for 1.2: ECDSA port, advisories, toolchain

Scope: the move of the vendored OpenMLS `openmls_basic_credential` and
`openmls_rust_crypto` (and the two compat copies of the 0.7.4 / 0.8.1 basic
credential) from RustCrypto `p256`/`p384` 0.13 to 0.14 (`ecdsa` 0.17), done to
drop `crypto-bigint` 0.5 (RUSTSEC-2026-0387). Reviewed line by line against the
upstream `openmls_rust_crypto` 0.6 / `openmls_basic_credential` 0.6 code and the
0.14 APIs. This is an internal review, not an external audit.

## Verdict

The port is equivalent to upstream in every security-relevant behaviour, with one
defect found and fixed (below). No interop difference with an independent
implementation (OpenSSL) was found. HPKE is unchanged.

## What was checked

| Area | Result |
|---|---|
| Scheme mapping | `ECDSA_SECP256R1_SHA256` is `p256::ecdsa` (SHA-256), `ECDSA_SECP384R1_SHA384` is `p384::ecdsa` (SHA-384), Ed25519 unchanged. The digest is implied by the curve type, so no mismatch is possible. |
| Signature encoding | DER on sign (`to_der`) and on verify (`from_der`), as RFC 9420 section 5.1.2 and upstream. A fixed-width `r\|\|s` signature, trailing bytes after the DER, truncated and empty input are rejected (tests). |
| Signing | Deterministic RFC 6979 (as upstream). The RFC 6979 A.2.5 P-256 "sample" vector reproduces exactly. |
| Key generation | `SigningKey::generate()` (OS RNG, `getrandom` feature) in the basic credential; the provider's own ChaCha DRBG (same generator as upstream, seeded from the OS) through a `rand_core` 0.6 to 0.10 shim in `signature_key_gen`. Rejection of the zero scalar is inside the library. Keys are distinct per call (test). |
| Public key encoding | Uncompressed SEC1: 65 bytes (P-256), 97 bytes (P-384), leading `0x04` (test). Parsing uses `from_sec1_bytes`: on-curve check, identity and wrong lengths rejected. As upstream (`EncodedPoint::from_bytes` + `from_encoded_point`), a compressed point is also accepted on verify. |
| Verification strictness | Same as upstream: `verify` does not reject a high-S signature (`r, n-s` verifies). That is not a signature-forgery issue and MLS does not use the signature as an identifier; pinned by a test so a change is noticed. Ed25519 still uses `verify_strict`. |
| Panics | Upstream used `SigningKey::from_bytes(key.into())`, which panics on a wrong-length key (`GenericArray::from_slice`). The port uses `from_slice` and returns an error for zero, out-of-range, empty and overlong keys (tests). |
| Constant time | No new arithmetic was written. Scalar and field operations are RustCrypto's constant-time `crypto-bigint` 0.7 / `primeorder`. |
| HPKE / KEM | Untouched: `hpke-rs` 0.8 with `hpke-rs-rust-crypto` 0.8, which now shares `p256` 0.14 with the signature code. DHKEM P-256 is exercised by the group round trip. |
| Compat copies | `compat_credentials/basic_credential_0_7` and `_0_8` differ from each other only by one `#[allow(deprecated)]`, use the same `from_slice` / `generate` / `to_sec1_point` calls as the main port, and now share its exact-width key check. They are test-only (migration tests). |

## Defect found and fixed

**Short private keys were accepted.** `SigningKey::from_slice` in p256/p384 0.14
left-pads input shorter than the field size, so a 31-byte (P-256) or 47-byte
(P-384) stored key would have signed as a different scalar instead of failing.
Upstream would have panicked on the same input. Keys come from our own storage,
so this was not remotely reachable, but a silently reinterpreted key is the
wrong failure mode. `OpenMlsCrypto::sign`, `Signer::sign` and the compat copies
now require the exact width (32 / 48 bytes) and return an error otherwise.

## Residual notes (no action needed)

- `SignatureKeyPair::new` for P-384 and the provider's key generation copy the
  secret scalar into a temporary that is not zeroized. Upstream does the same;
  the long-lived copy is held as `SecretVLBytes` / the sealed store.
- The web client's suite is `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`
  (`web/src/lib/media/groupTree.ts` in mnema-talk), which does not run the
  ported ECDSA code at all; the port matters for the P-256 suite and for
  anything signed with ECDSA. No test in this repository joins a `ts-mls`
  group from Rust.

## Tests added

- `experimental/vendor/openmls/openmls_rust_crypto/tests/ecdsa_interop.rs`:
  OpenSSL-generated keys and signatures (P-256/SHA-256, P-384/SHA-384) verify
  and re-sign deterministically; RFC 6979 A.2.5; tampering, wrong key, wrong
  curve, malformed DER, fixed-width signature, malformed public keys, invalid
  private keys; key generation shape; high-S parity.
- `experimental/vendor/openmls/basic_credential/src/lib.rs` (`ecdsa_tests`):
  P-256 and P-384 key pairs, DER output, exact-width key check.
- `experimental/vendor/openmls/openmls/tests/mnema_ecdsa_roundtrip.rs`: group
  create, Welcome join, application messages both ways, add commit, self
  update, removal, for the web suite (X25519/Ed25519) and the ECDSA P-256 suite.

CI runs them in the `windows-client` job. The existing OpenMLS suite for the
mandatory ciphersuites (`join`, `book_code`, ...) passes unchanged.

## Advisories

`cargo audit` cannot scope to a target, so the Linux-only crates showed up as
warnings forever. The check is now `cargo deny check advisories` with
`deny.toml`, whose graph is the Windows target (the only platform shipped while
macOS and Linux are paused). Every advisory (vulnerability, unmaintained,
unsound, yanked) fails the check unless it is in the graph; nothing is ignored
blanket.

- **Linux-only, out of the Windows graph:** `gtk` / `glib` 0.18 (GTK3,
  unmaintained; RUSTSEC-2024-0429 is also covered by the reviewed local
  backport), `proc-macro-error` (RUSTSEC-2024-0370, through `glib-macros`).
  `cargo tree -i gtk --target x86_64-pc-windows-msvc` finds no package in
  either the root or the `experimental/app` graph. The Electron Linux client does
  not use them. **Re-enabling the Tauri host on Linux needs Tauri's move to
  GTK4;** add the Linux target to `deny.toml` then.
- **One ignore, with reason (in `deny.toml`):** RUSTSEC-2026-0173,
  `proc-macro-error2`, a compile-time macro helper reached through
  `hpke-rs` 0.8.0 (latest) to `libcrux-sha3` to `core-models` 0.0.7 to
  `hax-lib` 0.3.7. It is in the experimental native-crypto crates only, which
  are not part of the shipped installer. `hax-lib-macros` 0.4.2 already drops it
  but `core-models` still requires `hax-lib` ^0.3.7, so it cannot be replaced
  from here; accepted as a scoped exception on 2026-10-25; re-check by 2027-01-25 and drop the ignore once `hpke-rs` / `core-models` move to `hax-lib` >= 0.4.
- `rusqlite` stays at 0.39 on purpose (0.40 bundles a SQLCipher that overflows
  the stack on Windows, see the note in `qualified-sdk/Cargo.toml`); `refinery`
  0.10.0 also caps `rusqlite` at 0.40.
- Test-only crates (e.g. `atomic-polyfill` in the vendored OpenMLS workspace's
  dev-dependencies) are excluded (`exclude-dev`).

## Toolchain and dependencies

Rust 1.99.1 is the current stable release; all `Cargo.lock` files were updated
(`cargo update`) and the one exact-pinned crate with a newer release outside a
documented hold, `zip`, moved to 9.1.0. npm dependencies of `ui`, `linux` and
`experimental/ui` have no newer release.

## TypeScript 7

TypeScript 7.0.2 is released, but is not adopted: `typescript-eslint` 8.72.0
(latest; `@typescript-eslint/typescript-estree` 8.72.0) declares the peer range
`typescript >=4.8.4 <6.1.0`, and TypeScript 7 is the native compiler without
the JavaScript compiler API those tools (and `vue-tsc` 3.3.13 through Volar)
load. The three trees stay on TypeScript 6.0.3, the latest 6.x. Revisit when
`typescript-eslint` publishes a release whose peer range includes 7.
