# Mnema Talk desktop

The desktop client for [Mnema Talk](https://github.com/msch128/mnema-talk), a
lightweight, self-hosted, single-server Discord alternative. It is built with
Tauri and wraps the same Vue web app as the browser, with two additions: it
removes server-served JavaScript from the trust equation, and it helps you stay
in a Talk while you play.

**Status: experimental, Windows first.** The browser app is the primary,
fully supported client. This client is an unsigned DEV build (it updates
itself from signed releases, see below), and its Windows/WebView2 acceptance (media permission prompts, gaming
surfaces, sealed media on real hardware) is still outstanding. See the
[product status](https://github.com/msch128/mnema-talk/blob/main/doc/product-status.md).

## What it adds to the browser

- **A signed web app instead of server code.** The client runs a signed web
  bundle that it verifies on every start (Ed25519, key pinned in the client).
  The server supplies data, never executable code, so a compromised server
  cannot inject JavaScript that reads your end-to-end encrypted media. The
  bundle is checked against the server's API version and a rising security
  floor, older bundles are refused, and updates come from the Mnema Talk
  releases. Design and threat model:
  [desktop-trusted-client.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md).
- **The session stays in the OS keychain** (Windows Credential Manager, macOS
  Keychain), not in a web profile.
- **Gaming integration (Windows).** A left-hand sidepeek shows who is in your
  Talk, who is speaking and who is sharing, and `Alt+M` opens an interactive
  overlay with Talk controls, while you have a connected Talk and a detected
  foreground game. Shortcuts for mute, deafen, push-to-talk and the overlay are
  configurable. See the
  [gaming guide](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-app.md#windows-gaming-dev)
  for setup and its limits (topmost windows, not exclusive fullscreen).

## Get it

Download `Mnema-Desktop-DEV_<version>_x64-setup.exe` from the
[latest release](https://github.com/msch128/mnema-talk-desktop/releases/latest)
and run it once. It installs for the current user only (no administrator
prompt) and updates itself from then on. SmartScreen warns because the installer
has no Authenticode certificate; the self-update does not depend on one (see
below). The portable ZIP of each release does not update itself. To build from
source, see [experimental/README.md](experimental/README.md#mnema-desktop-dev):
Rust and Node, with mnema-talk checked out next to this repository. The older
Windows DEV ZIP attached to
[mnema-talk releases through v0.7.3](https://github.com/msch128/mnema-talk/releases)
still loads the server's JavaScript and predates the signed bundle.

## Updates

Two things update, both from GitHub, both verified before use:

* **The web app** (the signed bundle) as described in
  [doc/desktop-trusted-client.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md).
* **The program itself** through the official Tauri updater. The client checks
  `releases/latest/download/latest.json` of this repository at every start and
  then every six hours (the same interval as the bundles). When a newer
  version exists, a small window offers **Update now** (download, install,
  restart), **On next start** (download now, install when the client is next
  started) and **Skip this version**; the menu entry **Update** reopens it. A
  failed attempt shows what went wrong and can be retried. `MNEMA_DESKTOP_NO_UPDATE_CHECK=1`
  turns both checks off.

An update is installed only if its minisign signature verifies against the
public key pinned in `experimental/app/tauri.conf.json`, the version the
signature was made for equals the announced version, and that version is newer
than the installed one. Where the file came from (GitHub, a mirror, a proxy)
decides nothing. The matching private key exists only as a secret of the
protected `desktop-updater-signing` environment (tags `v*`) and in the owner's
offline backup.

### Releasing

1. Raise the version in `experimental/app/tauri.conf.json`,
   `experimental/app/Cargo.toml` and `experimental/app/Cargo.lock` together
   (`scripts/check-release-version.mjs vX.Y.Z` checks this) and merge it to
   `main`.
2. Push the tag `vX.Y.Z` on that commit. `desktop-release.yml` re-tests, builds
   the tested executable into an NSIS installer, signs it in the protected
   environment, smoke-installs it per user on a Windows runner and publishes
   the installer, its `.sig`, `latest.json`, a portable ZIP and `SHA256SUMS`.
   The release is public only after everything is attached.

### Rotating or revoking the updater key

Installed clients trust exactly the key in their own `tauri.conf.json`.
To rotate: generate a new pair (`tauri signer generate`), publish a release that
**contains the new public key but is signed with the old key**, then switch the
secrets to the new key. Clients that took that release trust the new key from
then on. A release signed with a key a client does not have cannot be installed
by it. If the old key is lost or compromised and no release carrying the new
key reached a client, that client must install the new version manually from
the release page. Revoking a single release is done by publishing a higher
version; clients never go back to a lower one.

**Remote revocation.** The client also pins an offline root key (never in CI).
At start and every six hours it fetches a statement signed by that key from
`desktop-trust/` in the mnema-talk repository. The statement lists the web
bundle keys and updater keys that are allowed and the ones that are revoked;
the client keeps the last good one, refuses a lower serial, drops bundles
signed by a revoked key, and refuses updates signed by a revoked updater key.
If a CI key leaks, the owner issues a new statement instead of racing the
attacker with a release. The runbook is in mnema-talk
`doc/desktop-trusted-client.md` ("Key revocation"). A replacement updater key
listed in the statement is used to verify updates, so the key in
`tauri.conf.json` can be replaced without a client that has missed a release.

macOS and Linux builds do not exist yet. The updater code is platform neutral
(macOS would use `.app.tar.gz` updater artifacts and the same feed with a
`darwin-aarch64` or `darwin-x86_64` entry), but only Windows is built and
tested.

At startup, enter the HTTPS address of your community's instance. Microsoft
Edge WebView2 Runtime is required on Windows. Packages are unsigned, so
SmartScreen warns, and a checksum only detects a damaged download; it does not
prove who built it.

## Repository

| Path | What |
|---|---|
| `experimental/app` | The Tauri host: instance selector, bundle verification, proxy, keychain, gaming surfaces |
| `experimental/crates`, `experimental/vendor` | Native broker and crypto research, preserved but not selected by the shipped build |
| `src`, `ui`, `scripts` | The historical feasibility probe and its tooling (below) |
| `licenses`, `THIRD_PARTY_NOTICES.md` | Dependency licenses shipped with a build |

The Vue app itself lives in [mnema-talk](https://github.com/msch128/mnema-talk);
CI checks it out at `mnema-talk/` and embeds the signed bundle. Read the
server's [SECURITY.md](https://github.com/msch128/mnema-talk/blob/main/SECURITY.md)
before relying on any end-to-end claim. Licensed under the
[AGPL-3.0](LICENSE).

---

## Historical: Windows desktop feasibility probe

This directory contains the historical feasibility probe. The published
Windows Desktop DEV client is [experimental/app](experimental/README.md) in
`web-client` mode. See [product status](https://github.com/msch128/mnema-talk/blob/main/doc/product-status.md)
for downloads and actual runtime evidence. The limits below apply to the probe.

This is the first implementation stage of the desktop roadmap. It is **not a
released 0.7 client**: it does not log in, join Voice, capture media, create an
production overlay, or claim E2EE. Do not use it as a replacement for the current web app.

The Vue interface is bundled locally. Native Rust commands offer two operations:

- Explicit HTTPS discovery, with system certificate validation, no redirect
  following, no credentials, bounded response size/time and one request in flight.
- On Windows, foreground-only executable inspection against a user-selected
  list of basenames. Full paths and window titles are not sent to the interface
  or a server. Covering a monitor is **not** proof of exclusive fullscreen.

The independent gaming-policy tests cover Voice/trusted-media/game/input/lock
gating and held-PTT release. A native shortcut adapter polls only explicitly
configured keys and modifiers. It is not wired into Voice or the UI: short taps
can be missed and input-availability failures need an independent media watchdog.
These tests do not qualify runtime game input or GPU overlays.
An optional native synthetic fixture now exercises an owned child window, raw
configured input, session/watchdog gates and separate widget/overlay windows.
It does not connect to a server, microphone, real Voice or third-party game.
The probe reports Web API presence separately from actual media qualification.

## Build on Windows

Requires Node 26, Rust 1.99.0, the Visual Studio C++ build tools/Windows SDK and
WebView2. From the repository root:

```powershell
npm ci --prefix ui
npm --prefix ui run test
cargo test --locked --manifest-path Cargo.toml
npm run build
```

The unbundled probe executable is `target/release/mnema-desktop-probe.exe`.
Signing, installer packaging and production updates are later qualification
stages. This command does not produce an official signed release.
For local development: `npm run dev`.

Full checks require cargo-audit 0.22.2 (`cargo install cargo-audit --version
0.22.2 --locked`). The Desktop probe workflow runs these checks. The probe UI
typecheck extends `mnema-talk/web/tsconfig.base.json`, so check out
[mnema-talk](https://github.com/msch128/mnema-talk) at `mnema-talk/` before
`npm --prefix ui run check`. The small `go.mod` is only a module boundary
preventing a parent Go module from scanning npm Go fixtures.

Checks:

```text
cargo fmt --manifest-path Cargo.toml --check
cargo clippy --locked --manifest-path Cargo.toml --all-targets --features shell -- -D warnings
npm --prefix ui run build
```

Non-Windows hosts can build the probe UI/shell and test portable policy, but
foreground inspection explicitly reports an unsupported probe host. A cross-
target typecheck is not a Windows runtime test.

## Discovery candidate

The temporary G0 document is read from `/.well-known/mnema` on a root HTTPS
origin. Existing 0.6.1 servers do not implement it; their failure is expected.
This format is an experimental test contract, not the final versioned API:

```json
{
  "protocol": "mnema-desktop-discovery-v1",
  "community_id": "test-community",
  "api_versions": [1],
  "e2ee_required": true
}
```

Discovery success confirms document compatibility only. It neither establishes
honest server behavior nor authenticates devices and always leaves login disabled.
Subpath deployments are not supported by this initial probe. No TLS bypass or
HTTP fallback is provided.

## Windows game test

Enter the actual executable basename (not a path). Start foreground inspection,
switch to the game, then return to the probe. Its last recognized local game is
kept as diagnostic evidence; that record never enables a widget or an overlay.
Stop clears the record and sampling. No whole-system process scan occurs.

Record Windows build, CPU/GPU/driver, game/version, rendering API, display mode,
monitor/DPI and the observed result. Exclusive-fullscreen evidence requires a
separate verified rendering-mode test; window geometry cannot provide it.

Libraries and locked Rust dependency inventory: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
The app icons are generated from the project's existing `web/public/favicon.svg`.

## Distribution evidence

`node --test scripts/*.test.mjs` tests the development checksum receipt
and license inventory checker. `node scripts/collect-licenses.mjs --check`
verifies notices against the current locked dependency graphs and copied fonts.
Distribute the complete `licenses/` directory, this notice file and the root
`LICENSE` beside a development binary. Checksums are not publisher signatures;
receipts explicitly remain ineligible for the updater.

The Rust advisory scan currently reports no vulnerability-class advisories, but
retains `RUSTSEC-2024-0370` (unmaintained proc-macro-error) and
`RUSTSEC-2024-0429` (glib VariantStrIter unsoundness in the Linux shell dependency
graph). The latter blocks Linux distribution qualification pending a compatible
fix or reviewed reachability assessment. Neither package appears in the inspected
Windows or macOS graphs; this is not a clean cross-platform dependency claim.
No advisory is ignored.


## Explicit synthetic gaming fixture (Windows only)

The `gaming-fixture` feature adds native modules sharing the same opaque Rust
owner and shortcut types. A normal application launch keeps them inactive.
Start the synthetic route explicitly:

```powershell
npm ci --prefix ui
npm --prefix ui run check
cargo test --locked --manifest-path Cargo.toml --features gaming-fixture,custom-protocol --lib
cargo build --locked --manifest-path Cargo.toml --release --features gaming-fixture,custom-protocol
target/release/mnema-desktop-probe.exe --cooperating-game-session
```

The app starts one owned native child as a **synthetic game window**. F8 joins or
leaves its synthetic voice state. F9 toggles speaking, F10 muted, F11 sharing.
Only the foreground owned child plus joined synthetic voice permits the local
side widget. Alt+M requests the interactive overlay; a second Alt+M requests
return to that exact owned child. The normal Main window and unrelated apps do
not extend the game's foreground lease. Alt+Tab, minimize, close, lock, native
input loss and watchdog expiry revoke the context and release the synthetic PTT
latch. No actual microphone/session is created. Raw input is restricted to
configured primary keys and modifier classes; physical-up reconciliation only
releases retained state and never fabricates a press.

Configurable examples (launch arguments, not web/native renderer authority):

```powershell
target/release/mnema-desktop-probe.exe --cooperating-game-session --overlay-key=CTRL+SHIFT+X --ptt-key=MOUSE4 --fixture-no-widget
```

The fixture child grants foreground permission only to its independently
verified actual spawning parent, for the configured physical overlay chord while
foreground and joined. Real external game focus remains a single direct,
OS-permitted request, with no synthetic input, thread attachment or elevation.
The fixture's pipe and retained process creation identity carry no real media
proof. Its local widget uses bounded, sanitized synthetic names/statuses; its
renderer has event listening only and cannot activate native input/media.

The hosted **Windows desktop gaming fixture** workflow links an actual Windows
EXE and uploads it with an unsigned receipt and dependency notices. It does not
launch a GUI or promote the artifact to an updater/release. A successful build
proves compilation/linking, not the following still-required runtime checks:

- Widget present only in the owned game with joined synthetic voice; no widget
  after leaving, opening Main, Alt+Tab, minimizing, locking or terminating child.
- Repeated overlay/return, custom chords, held PTT and lost primary/modifier break;
  no press rearmed when context returns.
- WinEvent destruction/reuse, foreground changes while queued, monitor/DPI changes,
  raw-input registration conflicts with WebView2, native focus denial and cleanup.
- Renderer ordering/publication acknowledgement and hide failure; a GUI stall
  must not be mistaken for a qualified guarantee that old pixels disappear.

Double-checks, retained process handles and native events bound observed window
ownership but do not provide an atomic OS HWND lease. This candidate makes no
claim of real-game, anti-cheat, exclusive-fullscreen or production media/E2EE
qualification. Production gaming rendering remains explicitly disabled.


## Owned graphics mode fixture

A separate optional binary draws a synthetic scene, HUD and overlay inside its
own D3D11 swap chain. See [OWNED-D3D11-FIXTURE.md](OWNED-D3D11-FIXTURE.md) for
explicit build/run and rollback controls. `GetFullscreenState` reports our actual
DXGI state; Windows Fullscreen Optimizations can still change physical
presentation. It does not qualify an external Tauri window over true exclusive
fullscreen or a graphics companion inside third-party games.
