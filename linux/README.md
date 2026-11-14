# Mnema Desktop for Linux (Electron)

The Linux client of [Mnema Talk](https://github.com/mnema-xyz/talk). It does
what the [Tauri host](../experimental/README.md) does on Windows and macOS, with
the same security guarantees, on Chromium instead of the system WebView:
Tauri on Linux uses WebKitGTK, which has no WebRTC, so voice, camera and
screen sharing could not work there. Windows and macOS are not touched by
this directory.

Install and use: see the [main README](../README.md#linux). This file is for
people who change the client.

## What it guarantees

The same as the Tauri host (the design and threat model are in mnema-talk
[`doc/desktop-trusted-client.md`](https://github.com/mnema-xyz/talk/blob/main/doc/desktop-trusted-client.md)):

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
the app (HTTPS links open in the system browser, `mailto:` addresses in the mail program, after a confirmation),
permissions only for the app's own pages in the instance window (microphone,
camera, screen, notifications, clipboard write, output device, fullscreen;
microphone and camera only for the page itself, never for a frame inside it).
Unlike WebView2 and WKWebView on Windows and macOS, Electron shows no prompt of
its own once the host says yes, and X11 has no OS-level device prompt: a call
opens the microphone as soon as the signed app asks for it. No device
(USB, HID, serial, Bluetooth) ever, and `@electron/fuses`: no
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

Known difference from the Rust code, deliberate: the app CSP has no `ipc:`
entries (this client does not use network IPC).

Not differences, although an earlier version of this file said so: ZIP64 and
multi-disk archives are refused in both (a bundle larger than 4 GiB cannot exist
anyway: 64 MiB per file, 256 MiB in total); Ed25519 follows `verify_strict`
(`src/core/ed25519.ts`): Node's (OpenSSL) verifier does the arithmetic, and
around it a key must be a curve point that is not of small order, `R` must not
be of small order and `S` must be below the group order, as in `ed25519_dalek`;
and the proxy answers a redirect with 502 but passes a 304 on to the page,
which answers its conditional request.

## How it fits together

```
src/core        no Electron: runs in plain Node, where the tests run
  bundle.ts       zip + manifest + signature  (web_bundle.rs, verify)
  zip.ts          strict reader: stored/deflate, no ZIP64, no encryption, bounded inflate
  store.ts        verified bundles, cache, floor, high-water marks, selection  (web_bundle.rs, Store)
  trust.ts        trust statement, TrustState, updater key choice  (web_trust.rs)
  proxy.ts        serve the bundle, forward /api/, limits, response filter  (web_proxy.rs)
  download.ts     a download link of /api/ taken over by the host: streamed to disk, never held in memory
  downloadname.ts the saved file's name from Content-Disposition, the download attribute, the address
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

## Downloads

The proxy holds a whole answer in memory (one without a length up to 32 MiB), so
a download through it is not a way to save an export of up to 64 MiB, and
Chromium tells the person nothing when such a download fails. The host
therefore takes over the downloads of the app's own API. The preload (instance
pages only) catches a plain primary-button click on an `a[download]` whose
address is on the app origin under `/api/` and sends `desktop_download` with the
path and the suggested name; blobs, other addresses and clicks with a modifier
key are left to the page. The main process (`src/main/download.ts`) answers only
the instance page of a server, requests the path of **that** server with its
session (the proxy's request rules: its cookie, the instance's origin, only
`/api/`). It first asks where to save the file (a save dialog in the Downloads
folder) and makes the folder and the temporary file; only then is the server
asked, so a cancelled dialog or a folder that cannot be written leaves a data
export unspent on the server (it spends one once it sent the last byte). The
body is streamed into the temporary file next to the target, which is renamed
once the whole body has arrived; it is bounded by the server's own ceiling
(2048 MiB plus 1 MiB) and by an idle timeout per piece, and a failure removes
the partial file; a server that sends no answer within the idle time ends the
download. Partial files an earlier run left behind (only the client's own
names, `.` + 32 hex digits + `.mnema-download`, untouched for six hours) are
removed at the next download into that folder. The proposed name follows the `download`
attribute and the last path segment (the dialog comes before the server's
`Content-Disposition`; the Tauri host, which has no dialog, reads `filename*`
and `filename` first), cut down to one plain name that is safe on
every system: the part after the last `/` or `\`, control characters and
`< > : " | ? *` replaced by `_`, spaces and dots stripped from both ends (no
hidden file), "download" when nothing is left, at most 200 bytes with a short
extension kept, and a Windows device name (`CON`, `NUL`, `COM1`, ...) prefixed
with `_`. The proposed path must lie directly in the folder. What the person
picks in the save dialog is theirs. The cases are shared with the Rust host in
[`../test-vectors/proxy.json`](../test-vectors/proxy.json) (`download_names`).

A download ends in one of two ways: the file manager shows the file (and a
notice names it), or an error dialog says what failed and what to do next
(nothing left to download for a 404, the server's own message, cleaned and
capped, or the HTTP status, an interrupted transfer, a disk problem in plain
words). The sentences are the Tauri host's (`download_errors` in
`test-vectors/proxy.json`); a second download of the same server while one
runs is refused with one of them. Downloads Chromium starts itself (a blob) get the
same feedback when they end. The end-to-end run sets `MNEMA_DESKTOP_DOWNLOAD_DIR`
to save into a folder without a dialog; it is honoured only by an unpackaged
client and only together with `MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING=1`.

## Several servers side by side

One window: the server rail of the host on the left (the same page as in the
Tauri host), and next to it the page of one server or the page for adding one.
Each page is a `WebContentsView`. Any number of servers (there is no server
limit; the list file can be up to 8 MiB),
switched with Ctrl+1 to Ctrl+8 (the
first eight), Ctrl+9 (the last) or the rail, which scrolls when they do not fit,
added with Ctrl+Shift+N, reordered by dragging or Alt+arrow
and removed from the rail's menu. Servers can be grouped in folders (name,
colour, open or closed, kept across restarts) and renamed for this device; the
menu is the system's own popup (`Menu.popup`) and the questions (rename, folder,
remove) get a small window of their own, so a server's page can never cover
them. The community icon a server's Owner sets is fetched from that server only
(`GET /api/community/icon`, png or webp, at most 1 MiB, checked before it is
kept). No federation: one account per server.
A server is named on the rail, in the menu, in toasts and in the window title
by what it calls itself (`community_name` of `GET /api/health`, the name its
Owner chose or its host), shown as plain text; the address stands in for a
server that sends none or cannot be served.

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
* **Background servers** are hidden; a page is made only when its server is
  first opened and closed again after hiding for a while
  (`src/main/sleep.ts`, rules in `src/core/sleep.ts`, the same as the Tauri
  host's and shared in `test-vectors/rail-layout.json`: closed after 10
  minutes hidden while more than 4 run, never when the page says it holds a
  draft, an upload or a Talk, never when it has not said; settings `sleep` in
  the list file). A server without a page has its counts from the host
  (`src/core/feed.ts`: `GET /api/read-state` and `GET /api/dms`, read again on
  the socket's message, read, channel and conversation events; a 401 or 403
  clears them and ends the feed until the page is opened again; an
  unreachable server is asked again after 30 s, doubling up to 5 minutes, with
  jitter; at most every 2 seconds and at least
  every minute; the socket says the member is idle and nothing else; a rise of
  its mentions is one silent "N new mentions on <server>" notification, none
  for a muted server). At most 4 servers connect at once.
* **Update from the single-server client.** Its browser storage held every
  server it showed and recorded none, so it belongs to no one server: it is
  wiped once at the first start of this version and never handed on. Every server
  starts with a storage of its own.

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
| `MNEMA_DESKTOP_NO_UPDATE_CHECK=1` | no requests to GitHub (bundles, program); the trust statement (key revocation) is still fetched |
| `MNEMA_DESKTOP_DATA_DIR` | another data directory (tests use a temporary one) |
| `MNEMA_DESKTOP_NO_VAULT=1` | session in the private file, not the secret service |
| `MNEMA_DEBUG_PROTOCOL=1` | log what the client serves |
| `MNEMA_DESKTOP_DOWNLOAD_DIR` | end-to-end only: save downloads here without a dialog (unpackaged client with `MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING=1` only) |
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

**Not checked** (no such machine or device in CI): the Ctrl+1 to Ctrl+9 menu accelerators (1 to 8 the first eight, 9 the last) on a real desktop (the action behind them is unit-tested), a real Wayland session and
the PipeWire/portal screen picker, X11 screen sharing with real windows,
microphone and camera hardware, VA-API encoding, libsecret with a running
keyring (the fallback file was exercised), a private CA, a self-update against
a real GitHub release (the feed, signature, download and replacement are covered
by tests against a local server and by the Tauri CLI's own signatures, and the
release workflow verifies the feed before publishing), and long voice sessions.
