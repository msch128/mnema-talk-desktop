# Mnema Desktop DEV

The client uses Tauri to run the Vue web app from a **signed bundle** that it
verifies itself, instead of loading JavaScript from the selected instance.
Enter the HTTPS address of the instance at startup. The app is served from the
host's own origin (`mnema://localhost`, `https://mnema.localhost` on Windows);
the host forwards `/api/` to the instance, keeps the session cookie to itself
(in the macOS Keychain or Windows Credential Manager; a 0600 file only where the
system has none, and files from older versions move into the keychain)
and relays the WebSocket, so the instance supplies data and never code.
Normal production instances advertise `web_client_api` and `version` in
`/api/health`; no server change is needed. Design and threat model:
[doc/desktop-trusted-client.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md).

- **Bundle source.** One signed bundle (a release asset of mnema-talk,
  `mnema-web-<version>.zip`) is built into the executable; newer ones are
  downloaded from the mnema-talk GitHub releases (at most every six hours;
  `MNEMA_DESKTOP_NO_UPDATE_CHECK=1` turns the request off; the signed trust
  statement that revokes keys is still fetched) and kept in
  `<app data>/web-bundles/` (newest three). Every bundle is verified against
  the Ed25519 key pinned in `app/src/web_bundle.rs` on every start, so a
  modified cached file makes that bundle unusable.
- **Compatibility.** The bundle must speak the server's `web_client_api` and
  support its release (`min_server_version`). If the server is ahead the client
  looks for an update; otherwise it refuses with an explanation. There is no
  fallback to the instance's JavaScript.
- **Rollback.** Bundles below the highest `security_floor` of any verified
  manifest are refused; an update is installed only if it is newer.
- **Development.** `app/bundle/builtin.zip` (or `MNEMA_BUNDLE_ZIP`) is embedded
  at build time and must be signed with the pinned key; `MNEMA_REQUIRE_BUNDLE=1`
  fails the build without one. An instance on this machine may be entered as
  `http://127.0.0.1:<port>`; every other address must be HTTPS.

Download the installer for your platform from the
[desktop releases](https://github.com/msch128/mnema-talk-desktop/releases); the
[main README](../README.md#get-it) lists the files. Each release also has a
portable Windows ZIP that does not update itself. The server is supplied as a
Linux Docker image for amd64 and arm64. On Windows the client requires the
Microsoft Edge WebView2 Runtime.

The release workflow starts the actual Windows executable and checks that its
instance-selector window appears before publishing. That startup check also
reads the page's own origin, so WebView2 must present the custom protocol as
`https://mnema.localhost/`. It checks that the instance window leaves the
microphone on WebView2's own prompt and that the selector and the gaming
surfaces refuse one. Display capture uses that same host decision; this
check does not ask for it. The host passes a synthetic capture device as a WebView2
browser argument for that run, because the runtime replaces
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. The fake permission UI stays off,
so the host decision is what runs. Each probe window has its own WebView2
profile, because a refusal is remembered for the origin and then WebView2
does not ask again. The check does not click the prompt, and
a microphone grant is not part of the result.

**Several servers (1.1).** The window is a rail of the host on the left and the
page of one server next to it, as separate webviews (Tauri's `unstable`
multiwebview). Each server has its own webview with its own storage, its own
session and its own sockets; commands and the protocol handler pick the server
by the label of the calling webview (`i-<id>`), never by an argument. A
separate check, `scripts/multi-instance-selftest.mjs`, runs a build with the
test-only `multi-selftest` feature against two stand-in instances
(`scripts/fake-instance.mjs`) on Windows and macOS CI: storage, cookies, counters
and sockets of one server are not those of the other, Ctrl+1/2 switch, a report
from the background server raises its own badge, and removing a server wipes its
stored session and its browser storage. It uses the machine's own profile on
Windows, so it refuses to run outside CI there.
The check does not qualify a human permission prompt, a physical device,
sealed media, a real game, login, screensharing or E2EE.
A separate [published v0.7.1 Windows acceptance](https://github.com/msch128/mnema-talk/actions/runs/37888635202)
has since verified generated-account login, persisted chat, live peer receipt
without history fallback, and logout against an isolated normal server.
It used PostgreSQL 17.11 and in-memory test media storage. This does not qualify
S3 uploads, media capture, E2EE or games. [Product status](https://github.com/msch128/mnema-talk/blob/main/doc/product-status.md)
is the current evidence reference. The earlier acceptance runs predate the
signed bundle: the instance then supplied the web code. The verified-bundle
path has macOS development runs and unit tests. Windows CI checks the
custom-protocol origin, the media-permission decision and the gaming-surface
origin with synthetic devices. A human prompt, physical devices and sealed
media on real hardware are still outstanding.

Microphone/camera permission requests follow WebView2's browser permission flow.
HTTPS chat links to other sites open in the default Windows browser after an
explicit confirmation. Same-origin attachments open in a restricted child window sharing the app session;
the chat window remains open. Non-HTTPS
external links and custom URI schemes are blocked in this DEV build.
Voice uses the instance's browser path: DTLS-SRTP to the SFU, with microphone,
camera and screen payloads sealed in WebView2 before the SFU forwards them
(trust limits: mnema-talk `SECURITY.md`). Sealed media is not yet qualified
on WebView2.

Build the production web-client mode from this repository:

```sh
# From this repository's root, with mnema-talk checked out at mnema-talk/.
npm --prefix mnema-talk/web ci
npm --prefix mnema-talk/web run build
cargo build --locked --release --manifest-path experimental/app/Cargo.toml --features shell,web-client,custom-protocol
```

All desktop packages are unsigned DEV builds (no platform code-signing
certificate). The program updates itself from releases signed with the pinned
minisign key; see [Updates in the main README](../README.md#updates). The existing DeepFilterNet WASM transitive provenance
inventory remains incomplete; this is not a claim of final license closure.
Windows and macOS run this Tauri host. Linux runs the Electron client in
[`../linux`](../linux/README.md). Runtime qualification on macOS and Linux
hardware remains separate.

## Source layout

- `app/`: the Tauri host, its command permissions and capabilities.
- `app/page-tests/`: behaviour tests of the plain pages the host serves (the server rail and the update page).
- `licenses/`: the dependency license bundle shipped with a build, bound to `app/Cargo.lock` and the web app's `package-lock.json`.
- `../src/`: the gaming logic the host includes by path (`app/src/web_gaming.rs`).
- `../mnema-talk/web/`: the web app, checked out from mnema-talk.

From this repository's root, install the web app's dependencies with
`npm ci --prefix mnema-talk/web` and build it, then build the host with
`cargo build --locked --manifest-path experimental/app/Cargo.toml --features shell,web-client,custom-protocol`.
Builds use Rust 1.99.1 and the committed Cargo lockfile. The result is an
unsigned DEV executable without an automatic updater. Operating-system
toolchains are required for native dependency compilation.

The `Shared desktop client DEV builds` workflow builds this mode on Windows and
macOS. Matching pushes to `main` and `codex/**` produce short-lived unsigned DEV
artifacts with source revision/hash receipts and dependency notices. The managed
release workflow also calls it for a release tag; the release then carries the
installers, the signed update files and a portable ZIP (see "Releasing" in the
main README). Acceptance dispatches can instead exercise an existing published
EXE. Linking alone does not qualify runtime behavior or games.

The shared client rejects desktop call joins, automatic call restoration, camera
and screen sharing before capture or connection-state changes until the native
protected-media owner is integrated. Browser voice behavior remains available.
The browser's server-version reload banner is omitted for embedded desktop
assets, because reloading cannot install a new desktop build.

Existing vendored noise-filter assets retain their upstream notices; attribution
of every transitive component in that prebuilt WASM is still being verified
before binary distribution. No test account credentials, local database,
captured user data, native keychain, build artifact or private handoff belongs
in this directory.
