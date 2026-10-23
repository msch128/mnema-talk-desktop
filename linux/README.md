# Mnema Desktop for Linux (Electron)

The Linux client of [Mnema Talk](https://github.com/msch128/mnema-talk). It does
what the [Tauri host](../experimental/README.md) does on Windows and macOS, with
the same security guarantees, on Chromium instead of the system WebView:
Tauri on Linux uses WebKitGTK, which has no WebRTC, so voice, camera and
screen sharing could not work there. Windows and macOS are not touched by
this directory.

Install and use: see the [main README](../README.md#linux). This file is for
people who change the client.

## What it guarantees

The same as the Tauri host (the design and threat model are in mnema-talk
[`doc/desktop-trusted-client.md`](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-trusted-client.md)):

* It runs **only the signed web bundle** (`manifest.json`, `manifest.sig`,
  `app/...` in one zip; Ed25519), verified against keys pinned in the program.
  The instance supplies data, never code. There is no path that loads the
  instance's own JavaScript.
* **Trust statement** signed by the offline root key: allowed and revoked bundle
  and updater keys, `expires_at`, `max_version` caps, a serial that never goes
  back; revoked keys lose the rollback marks they raised.
* **Rollback protection**: a persisted high-water mark per `web_client_api` and
  per signing key, and a signed `security_floor`.
* **Compatibility** of bundle and server: exact `web_client_api`,
  `min_server_version`, never a fallback to server code; refuses with an
  explanation instead.
* **A proxy** serves `/api/` of the selected instance to the page on the app
  origin: the session cookie never enters the page, `Origin` is the instance's,
  answers that could run as a document become sandboxed downloads, bodies are
  bounded by the upload limit the instance announces. WebSockets are opened by
  the host.
* **Signed self-update** of the AppImage and of the `.deb` install with the same
  minisign key and rules. The `.deb` has its own feed keys (`linux-x86_64-deb`,
  `linux-aarch64-deb`) that Tauri clients and the AppImage never read. The
  package is verified (signature, version bound into it, strictly newer) before
  anything else, and written to a private `0700` temporary directory. The
  privileged step (`pkexec /bin/sh -c <fixed script>`, arguments, no shell
  interpolation) does not trust that file: it reads it (regular file, no symlink,
  at most the expected length) into a `0600` file in a root-owned `0700`
  directory, checks the SHA-256 of the verified bytes on the copy and installs
  only the copy (`apt-get install -y`, `dpkg -i` if apt-get fails), so the user
  account cannot swap the package while the polkit prompt is open; a mismatch
  installs nothing. The software center is opened via `xdg-open` if pkexec is
  missing.
  "Install on next start" asks before the installer's password prompt appears,
  and runs before any window opens. A `.deb` install is recognised by running
  from `/opt` without `APPIMAGE`.

Added by Electron: context isolation, the renderer sandbox (a start without it is
refused), no Node in any page, a CSP of the Mnema SPA, a privileged secure
scheme (`mnema://localhost`), no network from any page (`webRequest` cancels
everything but `mnema:`, `blob:`, `data:`), no navigation or new window outside
the app (HTTPS links open in the system browser after a confirmation),
permissions only for the app's own pages in the instance window (microphone,
camera, screen, notifications, clipboard write, output device, fullscreen), no
device (USB, HID, serial, Bluetooth) ever, and `@electron/fuses`: no
`ELECTRON_RUN_AS_NODE`, no `NODE_OPTIONS` or `NODE_EXTRA_CA_CERTS`, no
`--inspect`, cookie encryption, only load the app from `app.asar`. A packaged
client also refuses a remote debugging port. The ASAR integrity fuse is set
too, but Electron enforces it on macOS and Windows only, not on Linux; on Linux
the AppImage is read-only and the `.deb` is root-owned.

## Why the verification is ported, not shared

Options were a native helper built from the Rust modules, or a port to
TypeScript with shared vectors. The port won:

* The Rust modules cannot be built without Tauri today (`web-client` needs the
  `shell` feature), and a helper would need its own Rust build for two
  architectures, a second binary next to the asar (which Electron's integrity
  check does not cover) and a process boundary for every request.
* Everything the client trusts then lives in one asar: the TypeScript is what
  the fuses, the sandbox and a reviewer see.
* The port is checked, not trusted: [`../test-vectors/`](../test-vectors) holds
  the cases (signed bundles and their tamperings, rollback scenarios, trust statements, proxy rules, signatures)
  and **both** implementations read the same files. `npm run vectors` writes them
  from the TypeScript implementation; the Rust tests (`shared_vectors`,
  `shared_minisign_vectors`) and [`test/vectors.test.ts`](test/vectors.test.ts)
  must give the same answers, and CI fails on drift in either language. A change
  in the Rust rules shows up as a failing vector test here, and the other way
  round. The real signed bundle of a release and the real trust statement
  also verify with the TypeScript code (the latter is a unit test, the former the
  end-to-end run).

Known differences from the Rust code, all deliberate (the proxy answers 502 for every 3xx, 304 included, like the Rust host):

* The app CSP has no `ipc:` entries (this client does not use network IPC).
* ZIP64 and multi-disk archives are not bundles; a bundle larger than 4 GiB cannot
  exist anyway (limits are 64 MiB per file and 256 MiB in total).
* Ed25519 signatures are verified with Node's (OpenSSL) verifier, not
  `verify_strict`; the keys are pinned or root-signed, so the small-order cases
  strictness guards against cannot occur. Keys are still checked to be valid
  curve points when they are parsed.

## How it fits together

```
src/core        no Electron: runs in plain Node, where the tests run
  bundle.ts       zip + manifest + signature  (web_bundle.rs, verify)
  zip.ts          strict reader: stored/deflate, no ZIP64, no encryption, bounded inflate
  store.ts        verified bundles, cache, floor, high-water marks, selection  (web_bundle.rs, Store)
  trust.ts        trust statement, TrustState, updater key choice  (web_trust.rs)
  proxy.ts        serve the bundle, forward /api/, limits, response filter  (web_proxy.rs)
  cookies.ts      the jar, vault or 0600 file  (web_proxy.rs, web_vault.rs)
  sockets.ts      /api/ws relay  (web_ws.rs)
  updates.ts      newer bundles and the trust statement from GitHub  (web_updates.rs)
  instances.ts    the servers side by side: slots, counters, the rail's state, the list at rest  (instances.rs)
  trusted.ts      the state of the client: store, keys, statement, the servers' sessions  (web_client.rs)
  client.ts       address, health, links, app routes  (web_client.rs, local_route.rs)
  minisign.ts     package signatures  (native_updates.rs, verify_package)
  selfupdate.ts   check / download / verify / install now or later  (native_updates.rs)
src/main        Electron: the window with the rail and the pages, sessions, protocol, IPC, menus
  shell.ts        the window: rail, page for adding a server, one page per server  (web_shell.rs)
  sessions.ts     one session and one protocol handler per server, bound to its id
  rail.ts         the commands of the rail, on a channel of their own
src/preload     the scripts next to the pages: the app bridge, and the rail's own
assets          the update window (the Tauri host's page, plus the .deb case), the screen picker and the server rail
                (`rail-ui` is the same file set as `experimental/app/rail-ui`; a test keeps them equal)
```

**The web app is not changed.** It was written for the Tauri host: it imports
`@tauri-apps/api/core` and reads `window.isTauri`, `__TAURI_INTERNALS__`
(`invoke`, `transformCallback`, `Channel`) and `__MNEMA_TRUSTED_APP__`. The
preload ([`preload.cts`](src/preload/preload.cts)) provides exactly that surface
over one IPC channel, so the signed bundle runs as built. The main process
answers `desktop_open_instance`, `desktop_ws_open/send/close` and
`native_update_state/action` (the names and shapes of the Tauri host), decides
by the kind and URL of the calling frame whether it may, and nothing else exists;
the gaming commands answer "Unknown command" (there is no overlay on Linux).
A host-neutral `isTrustedDesktop()` in the web app would remove the need for the
shim; it would need a new bundle and is not required.

Each kind of window has its own session (rail, selector, update, picker), and
so has every server and its attachment window, so what a page can fetch does not
depend on a label it could claim. The server sessions are persistent: the app
keeps the state of its end-to-end encryption in the browser storage of that
partition, as on the other platforms.

## Several servers side by side

One window: the server rail of the host on the left (the same page as in the
Tauri host), and next to it the page of one server or the page for adding one.
Each page is a `WebContentsView`. Up to nine servers, switched with Ctrl+1 to
Ctrl+9 or the rail, added with Ctrl+Shift+N, reordered by dragging or Alt+arrow
and removed from the rail's menu. No federation: one account per server.

How the servers are kept apart:

* **Storage.** Every server has its own persistent session partition
  (`persist:inst-<id>`, the id being the first 16 hex digits of the SHA-256 of
  its origin), so local storage, IndexedDB (the end-to-end state) and caches of
  one are not those of another. The attachment window of a server has its own
  partition too.
* **Session cookie.** It lives in the host, one jar per server, in the secret
  service (or its 0600 file) under the server's id; a page only ever gets the
  cookie of its own server. The `mnema:` handler of a server's partition is made
  for that server alone, so a page cannot reach another server's API.
* **Commands.** The server a command acts for is the one the calling page was
  made for, taken from the host's own record of the page, never from an
  argument. The rail has a preload and a channel of its own and is answered only
  for its own page; the pages of servers cannot name those commands.
* **WebSocket relay.** One budget per server; only `/api/ws` and
  `/api/ws?dm=1` can be opened, with that server's cookie and origin.
* **Counters.** A page reports `desktop_report_unread` (unread channels,
  mentions); an older bundle's `desktop_set_badge` stands in as mentions until
  the page reports. The launcher badge is the sum of the mentions. A toast names
  its server once there are several; using it brings that server forward and
  reaches only its page.
* **Removal** takes the server out of the list and wipes its stored session, its
  partition (storage, cache and directory), its sockets, its toasts and its
  attachment window. A directory the engine still holds is removed again later,
  and leftovers are swept at the next start.
* **Background servers** are hidden, not stopped: their sockets and counters stay
  alive.
* **Update from the single-server client.** The old browser storage (end-to-end
  state) goes to the one server whose session was the only one stored. If that
  cannot be told (none, or several servers were stored), it is wiped instead of
  being handed to a server that may not have owned it.

## Develop

Node 26. Dependencies are installed without scripts; Electron's download runs on its own.

```sh
cd linux
npm ci --ignore-scripts && node node_modules/electron/install.js
(cd .. && BUNDLE_DIR=linux/bundle bash scripts/stage-web-bundle.sh)   # the signed bundle; needs gh
npm run check          # lint, types, vectors, unit tests, build
npm run start          # build and run (set MNEMA_DESKTOP_DEVTOOLS=1 for developer tools)
npm run e2e            # a real Electron against a fake instance; needs a display (xvfb-run -a on a server)
npm run vectors        # rewrite ../test-vectors after changing a rule (then run the Rust tests)
npx electron-builder --linux dir AppImage deb --x64 --publish never   # release/
```

Without `bundle/builtin.zip` the client still starts and has to download a bundle.

| Variable | Effect |
|---|---|
| `MNEMA_DESKTOP_NO_UPDATE_CHECK=1` | no background checks (bundles, trust statement, program) |
| `MNEMA_DESKTOP_DATA_DIR` | another data directory (tests use a temporary one) |
| `MNEMA_DESKTOP_NO_VAULT=1` | session in the private file, not the secret service |
| `MNEMA_DEBUG_PROTOCOL=1` | log what the client serves |
| `MNEMA_DESKTOP_ALLOW_NO_SANDBOX=1` | start although the sandbox is off |
| `MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING=1` | let a packaged client open a debugging port (the end-to-end run needs it) |
| `MNEMA_DESKTOP_DEVTOOLS=1` | developer tools in an unpackaged run |
| `MNEMA_E2E_APP` | run the end-to-end suite against this packaged executable |

**Screen-share audio is not available on Linux** (Electron captures system audio only on Windows and macOS); screen video, microphone and camera work.

## What has been checked, and what has not

Checked in CI and locally: unit tests, shared vectors in TypeScript and Rust,
the real trust statement and the real signed bundle, minisign signatures written
by the Tauri CLI, the end-to-end suite against the unpackaged app (macOS and
Linux) and against the **packaged** build under Xvfb in Ubuntu 24.04 (arm64
container; x86_64 and arm64 runners in CI): the verified bundle is served in a
secure context with `RTCRtpScriptTransform` (what the app's media guard needs,
so its fail-closed check passes), an instance opens, the proxy and the WebSocket
relay behave as specified, permissions and navigation are locked, the fuses are
set, the unsandboxed start is refused. The AppImage and the installed `.deb` start
and show the selector.

The two-server end-to-end run (`test/e2e/multi.e2e.ts`) signs in to two stand-in
instances and checks storage, cookies, sockets, counters, switching, reordering and
removal against the real app.

**Not checked** (no such machine or device in CI): the Ctrl+1 to Ctrl+9 menu accelerators on a real desktop (the action behind them is unit-tested), a real Wayland session and
the PipeWire/portal screen picker, X11 screen sharing with real windows,
microphone and camera hardware, VA-API encoding, libsecret with a running
keyring (the fallback file was exercised), a private CA, a self-update against
a real GitHub release (the feed, signature, download and replacement are covered
by tests against a local server and by the Tauri CLI's own signatures, and the
release workflow verifies the feed before publishing), and long voice sessions.
