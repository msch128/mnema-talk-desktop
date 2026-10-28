# Mnema Talk desktop

The desktop client for [Mnema Talk](https://github.com/msch128/mnema-talk), a
lightweight, self-hosted, single-server Discord alternative. It wraps the same
Vue web app as the browser, with two additions: it removes server-served
JavaScript from the trust equation, and it helps you stay in a Talk while you
play. Windows and macOS run it in a Tauri host. Linux runs it in Electron,
because the WebKitGTK that Tauri uses on Linux has no WebRTC.

**Status: Beta, unsigned.** The
browser app is the primary, fully supported client. This client is Beta: usable
and maintained, but not signed with a platform certificate, so Windows
SmartScreen and macOS Gatekeeper warn on first start (steps below), and it has
not been qualified on real hardware for macOS and Linux. It updates itself from
signed releases, see below. Windows CI checks the WebView2
custom-protocol origin, which window may ask for a microphone, and the gaming
surfaces. The host passes a synthetic capture device as a WebView2 browser
argument for that run, because the runtime replaces
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. The instance window leaves the
request on WebView2's own prompt, which the check does not click, and the
other windows are refused. A human permission prompt, physical devices and
sealed media on real hardware are still outstanding. The macOS build has
had less testing still (see [macOS](#macos)). See the
[product status](https://github.com/msch128/mnema-talk/blob/main/doc/product-status.md).
There is no external security audit.

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
  Keychain, the secret service on Linux), not in a web profile.
- **OS notifications and an app badge.** Messages that would raise a browser
  notification show as native toasts instead (Windows, macOS, Linux), and the
  unread mention count appears on the taskbar, dock or launcher icon. Clicking
  a toast opens the window at its message. On macOS the system asks once
  whether Mnema may show notifications (UserNotifications); on Windows the
  installer's Start-menu shortcut registers the app for toasts. The instance
  window announces the `notify` capability; web bundles from before this
  feature keep working with the browser's own notifications.
- **Several servers side by side (1.1).** A rail on the left lists your servers
  with an unread dot and a mention count each. Add as many as nine, switch with
  Ctrl+1 to Ctrl+9 (Cmd on macOS), drag or Alt+arrow to reorder, and remove one
  from its menu (right-click or Delete). The same menu sets your notification
  level for that server (all messages, only mentions, mute) on servers that
  support it (Mnema Talk 1.2 and newer). One account per server, no federation:
  every server has its own session in the keychain, its own browser storage
  (a WebView2 profile per server on Windows, a data store per server on macOS
  14 and newer, a session partition on Linux) and its own connection. Removing a
  server wipes its stored session and its local data from this device. The
  taskbar or dock badge is the sum of the mentions; a toast names its server
  and brings it to the front when used. Servers in the background keep running
  (hidden, not stopped), so their counts and calls stay live. Overlay and
  shortcuts of the gaming integration belong to the server that owns the Talk
  you are in. **Updating from the single-server client (1.1 release note):** that client kept every server in one shared storage and did not record which one it showed, so the storage belongs to no one server. On the first start of this version it is wiped, once, and no server inherits it: you sign in again, and the end-to-end media gets a new device key (messages live on the server and are unaffected). The host's own pages use a separate storage. On macOS before 14 the data stores cannot be separated, so the
  client runs one server there. Servers on different versions work side by side:
  each runs the verified web app that fits it, and one that is too old or too
  new says so in its own tab. See
  [desktop-trusted-client.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md#several-servers-at-once).
- **Gaming integration (Windows).** A left-hand sidepeek shows who is in your
  Talk, who is speaking and who is sharing, and `Alt+M` opens an interactive
  overlay with Talk controls, while you have a connected Talk and a detected
  foreground game. Shortcuts for mute, deafen, push-to-talk and the overlay are
  configurable. See the
  [gaming guide](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-app.md#windows-gaming-dev)
  for setup and its limits (topmost windows, not exclusive fullscreen).
  Game detection and push-to-talk are event driven (no polling, no process
  enumeration, no injection, no keyboard hook); [GAMING.md](GAMING.md) is the
  reference, including the anti-cheat safety statement. The gaming integration is Windows
  only. macOS and Linux ship the client without it.

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

### macOS

Download `Mnema-Desktop-DEV_<version>_aarch64.dmg` from the
[latest release](https://github.com/msch128/mnema-talk-desktop/releases/latest)
(Apple silicon, M1 or newer, macOS 12 or newer), open it and drag the
app to Applications. It updates itself from then on.

The app is **ad-hoc signed, not notarized**: there is no Apple Developer
account behind it, so Gatekeeper refuses it on the first start ("cannot be
opened because Apple cannot check it for malicious software"). Pick one:

* **macOS 15 and newer:** try to open the app once, then open *System Settings
  → Privacy & Security*, scroll to the message about Mnema Desktop DEV and
  choose **Open Anyway**.
* **macOS 14 and older:** right-click the app and choose **Open**, then **Open**
  again.
* **Terminal:** `xattr -dr com.apple.quarantine "/Applications/Mnema Desktop DEV.app"`

This is needed once: the self-update downloads inside the app and does not
mark the new version as quarantined, so later versions start without it. The
update is still verified by the pinned minisign key (see below); Gatekeeper
plays no part in that. The first time you speak, share the camera or share the
screen macOS asks for the microphone, camera or screen recording permission
(*System Settings → Privacy & Security* lists them for Mnema Desktop DEV).
Because every version is a new ad-hoc signature, macOS may also ask once after
an update whether the app may use its keychain entry; if you decline, you sign
in again. There is no gaming overlay on macOS.

### Linux

Download from the [latest release](https://github.com/msch128/mnema-talk-desktop/releases/latest)
the file for your machine, x86_64 (also called amd64) or arm64:

* **AppImage**, `Mnema-Desktop-DEV_<version>_x86_64.AppImage` or `..._arm64.AppImage`:
  make it executable (`chmod +x`) and run it. It needs no installation and no
  FUSE, and it updates itself (see [Updates](#updates)).
* **.deb**, `Mnema-Desktop-DEV_<version>_amd64.deb` or `..._arm64.deb`, for
  Debian and Ubuntu: `sudo apt install ./Mnema-Desktop-DEV_<version>_amd64.deb`.
  It installs to `/opt/Mnema Desktop DEV` with a menu entry and an AppArmor
  profile, and is updated by downloading a newer package; the client tells
  you when one exists and links to the release page.

At the first start enter the HTTPS address of your community's instance, as on
the other platforms. Your data (the verified web apps, the trust statement, the
browser storage of the app) lives in `~/.config/mnema-desktop`; `MNEMA_DESKTOP_DATA_DIR`
points the client somewhere else.

**Why Electron.** The Tauri host would need WebKitGTK on Linux, which has no
WebRTC, so voice and screen sharing could not work. Electron ships Chromium. The
verification of bundles, the trust statement, the rollback marks, the proxy and
the session cookie are a port of the Tauri host to TypeScript, run against the
same test vectors as the Rust code (`test-vectors/`); see [linux/README.md](linux/README.md)
for the design. Windows and macOS are unchanged.

**The sandbox.** Chromium isolates the page in a sandbox that needs
unprivileged user namespaces. Ubuntu 24.04 and newer restrict them by default
(AppArmor), and the AppImage launcher then silently drops the sandbox. The
client **refuses to start without the sandbox** and says so. Use the `.deb`
(its AppArmor profile permits the app), or allow namespaces
(`sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`), or accept the
risk with `MNEMA_DESKTOP_ALLOW_NO_SANDBOX=1`.

**The session.** The session cookie is kept encrypted with a key in the secret
service (GNOME Keyring, KWallet, anything that speaks libsecret). Without a
secret service Chromium would only use a fixed password, which is no protection,
so the client falls back to a private file (mode 600) in its data directory, as
the other platforms do where no keychain exists: copies of the profile and
backups that cannot read that file do not carry the session, but another
program of your user can. `MNEMA_DESKTOP_NO_VAULT=1` chooses the file on
purpose.

**Wayland and screen sharing.** The client asks Chromium to pick Wayland on a
Wayland session (`--ozone-platform-hint=auto`) and X11 otherwise. On Wayland, screen sharing goes
through PipeWire and the desktop portal: you need `xdg-desktop-portal` and the
backend for your desktop (`xdg-desktop-portal-gnome`, `-kde` or `-wlr`) and
PipeWire; the portal's own dialog asks which screen or window to share. On X11
the client shows its own list with previews. Sharing the **sound** of a screen
is not available on Linux (Electron captures system audio only on Windows and
macOS); the microphone and camera work as in any Chromium. Hardware video
encoding is Chromium's, and is off on many Linux setups; where the driver
supports it, `--enable-features=VaapiVideoEncoder` turns on VA-API.

**Private certificate authorities.** Certificates are checked against the
system store, like the other platforms, so a community on a private CA works
once the CA is installed system-wide (`update-ca-certificates` on Debian and
Ubuntu). `NODE_EXTRA_CA_CERTS` is deliberately switched off in the program.

**Not available.** The gaming overlay is Windows only. `MNEMA_DEBUG_PROTOCOL=1`
logs what the client serves. A packaged client does not open a remote debugging
port and does not accept `ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS` or `--inspect`.

## Updates

Two things update, both from GitHub, both verified before use:

* **The web app** (the signed bundle) as described in
  [doc/desktop-trusted-client.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md).
* **The program itself** through the official Tauri updater (Windows, macOS)
  or, on Linux, the same signed feed read by the Electron client (below). The client checks
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
   `experimental/app/Cargo.toml` and `experimental/app/Cargo.lock`, and in
   `linux/package.json` and `linux/package-lock.json`, together
   (`scripts/check-release-version.mjs vX.Y.Z` checks this) and merge it to
   `main`.
2. Push the tag `vX.Y.Z` on that commit. `desktop-release.yml` re-tests, builds
   the tested executable into an NSIS installer, signs it in the protected
   environment, smoke-installs it per user on a Windows runner, checks that
   `latest.json` verifies against the pinned key and publishes the installer,
   its `.sig` file, `latest.json`, a portable ZIP, the macOS disk image and
   updater archive, the Linux AppImages and `.deb` packages, and `SHA256SUMS`.
   The release is public only after everything is attached. The Tauri host on
   Linux is not built.

To rehearse a release without publishing, run `desktop-release.yml` by hand
(`gh workflow run desktop-release.yml --ref main -f version=X.Y.Z`). It runs
the same jobs and checks for that version, creates no release and moves no tag,
and uploads the files, `latest.json` and `SHA256SUMS` as the artifact
`mnema-desktop-rehearsal`. It never enters the signing environment: each signing
job makes a throwaway key, and the artifact carries only their public halves
(`rehearsal-updater-*.pub`), which the feed is verified against.

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
the client keeps the last good one, refuses a lower serial or an expired
statement, drops bundles signed by a revoked key (the rollback marks a revoked key
raised are cleared, those of trusted keys stay; a statement can also cap them
with `max_version`), and refuses updates signed by a revoked updater key. A statement is
valid for 180 days; while the one in force has expired, or its fetch failed in
a round, the client installs nothing new and keeps running what it has. A
client only counts a statement as fresh after fetching it in the current run. An installer kept for the next start is applied at startup, before the main window loads, after that fetch succeeds and the installer has been re-verified against the fresh statement's keys; if the fetch fails it stays kept and the next start tries again.
**The owner must re-sign about every 3 months** (`trust sign -renew`, see the
mnema-talk doc). If a CI key leaks, the owner issues a new statement instead of racing the
attacker with a release. The runbook is in mnema-talk
`doc/desktop-trusted-client.md` ("Key revocation"). A replacement updater key
listed in the statement is used to verify updates, so the key in
`tauri.conf.json` can be replaced without a client that has missed a release.

On macOS the updater replaces the app in place from the signed
`.app.tar.gz`; `latest.json` carries a `darwin-aarch64` entry.

On Linux the feed carries `linux-x86_64` and `linux-aarch64` entries that name
the signed AppImages. Tauri clients look up their own platform and ignore these
(the entries use the same fields, so nothing in the file changed for them). The
Electron client applies the same rules as the Tauri updater, in TypeScript: the
minisign signature must verify against the pinned key (or a replacement key
the trust statement lists, and never a revoked one), the version recorded inside
the signature must equal the announced one, the version must be newer than the
running one, the download must be an HTTPS URL on `github.com`, and nothing new
is installed without a trust statement fetched in the current run. The window
offers the same choices (**Update now**, **On next start**, **Skip this version**).
**Update now** replaces the AppImage file it was started from (a failed write
leaves the installed one untouched) and restarts it; **On next start** keeps the
verified AppImage and installs it, after verifying it again, at the next start.
A `.deb` installation is owned by the package manager: the client announces
the version and links to the release page instead. The AppImage has to be in a
directory you can write to.

At startup, enter the HTTPS address of your community's instance. Microsoft
Edge WebView2 Runtime is required on Windows; macOS uses the system WebKit;
Linux brings its own Chromium. Packages are unsigned, so
SmartScreen warns, and a checksum only detects a damaged download; it does not
prove who built it.

## Repository

| Path | What |
|---|---|
| `experimental/app` | The Tauri host: instance selector, bundle verification, proxy, keychain, gaming surfaces |
| `linux` | The Linux client (Electron): the same verification, proxy and updater in TypeScript; AppImage and .deb |
| `test-vectors` | Cases shared by the Rust host and the Linux client: bundles, rollback marks, trust statements, proxy rules, minisign |
| `src` | The gaming logic (foreground tracking, game matching, input state machines) the host includes by path; the root crate builds it on its own so its tests run on every system |
| `ui` | Pins the Tauri CLI that the workflows use to bundle the installer |
| `scripts` | Build, release and verification tooling and its tests |
| `experimental/licenses`, `THIRD_PARTY_NOTICES.md` | Dependency licenses shipped with a build |

The Vue app itself lives in [mnema-talk](https://github.com/msch128/mnema-talk);
CI checks it out at `mnema-talk/` and embeds the signed bundle. Read the
server's [SECURITY.md](https://github.com/msch128/mnema-talk/blob/main/SECURITY.md)
before relying on any end-to-end claim. Licensed under the
[AGPL-3.0](LICENSE).

---

## Checks

```text
cargo test --locked --manifest-path Cargo.toml
cargo clippy --locked --manifest-path Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path experimental/app/Cargo.toml --features shell,web-client,custom-protocol
cargo clippy --locked --manifest-path experimental/app/Cargo.toml --features shell,web-client,custom-protocol --all-targets -- -D warnings
node --test scripts/*.test.mjs
```

The first two commands build only the gaming logic of `src/`. The host and the
page tests (`experimental/app/page-tests`) run in the *Shared desktop client DEV
builds* and *Desktop checks* workflows. The small `go.mod` is only a module
boundary preventing a parent Go module from scanning npm Go fixtures.

Libraries and locked Rust dependency inventory: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
The app icons are generated from the project's existing `web/public/favicon.svg`.

## Distribution evidence

`node --test scripts/*.test.mjs` tests the license inventory checker.
`node experimental/scripts/collect-licenses.mjs --check` (with
[mnema-talk](https://github.com/msch128/mnema-talk) checked out at `mnema-talk/`
and `npm ci` run in `mnema-talk/web`) verifies the notices in
`experimental/licenses` against the locked dependency graph of the host and the
web app's production packages. Distribute that directory, the notice file and
the root `LICENSE` beside a development binary. Checksums are not publisher
signatures.

The Rust advisory scan (`cargo deny`, `deny.toml`) is scoped to the shipped
Tauri targets, Windows and macOS. The published Linux client is Electron and is not in that graph. The
Linux-only GTK3 crates (`gtk`, `glib` 0.18, `proc-macro-error`) stay out of it;
the Tauri host on Linux is not built. Details:
[doc/security-review-1.2.md](doc/security-review-1.2.md).
