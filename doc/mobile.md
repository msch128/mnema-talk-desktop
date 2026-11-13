# Mobile apps: design and preparation

Status: **preparation**, not a product. Nothing here is signed with a real
identity, published to a store or offered for download. The browser app with
its phone layout and the installable web app (PWA) stay the supported way to
use Mnema Talk on a phone until the phases below are done.

This document covers native Android and iOS apps next to the desktop client:
store apps that a member installs, points at their community's server, and
uses like the desktop client. They follow the Mnema design language (calm,
quiet, never a copy of another chat product's look), exactly as the desktop
client does: the app shows the same verified web bundle, and the native parts
are as small and as plain as the platform allows.

The spike in [`mobile/`](../mobile) proves the build; its state is in
[Spike](#spike-what-was-proven).

## Contents

- [Goals and non-goals](#goals-and-non-goals)
- [Architecture](#architecture)
- [Alternatives compared](#alternatives-compared)
- [Day one and native work](#day-one-and-native-work)
- [Push notifications](#push-notifications)
- [Calls and audio](#calls-and-audio)
- [Invite links and deep links](#invite-links-and-deep-links)
- [Store requirements](#store-requirements)
- [Signing and what the owner must decide](#signing-and-what-the-owner-must-decide)
- [CI](#ci)
- [Phases and estimates](#phases-and-estimates)
- [Spike: what was proven](#spike-what-was-proven)

## Goals and non-goals

Goals:

- The same promise as the desktop client: the phone runs a **signed web
  bundle** that the app verifies (Ed25519, key pinned in the app), the server
  supplies data and never code, and the session cookie stays with the host,
  never in the page.
- Share the host core with the desktop client instead of writing it a third
  time (the Linux client is already a TypeScript port held to the same test
  vectors).
- No central service of ours that every server depends on. A self-hosted
  server keeps working without anything the project runs.
- Push notifications that arrive when the app is closed, without a third party
  reading the text.

Non-goals for the first versions: a different user interface from the web app,
screen sharing from the phone, features the browser app does not have, text
end-to-end encryption (decided against for the product as a whole).

## Architecture

**Recommendation: Tauri 2 mobile, sharing the Rust host core of
`experimental/app` by path, with a small mobile shell of its own.**

```
                 +------------------- phone -------------------+
                 |  system WebView (Android WebView / WKWebView) |
                 |    verified web bundle at the app origin      |
                 |    https://mnema.localhost (Android)          |
                 |    mnema://localhost (iOS)                    |
                 +---------|----------------------|--------------+
                   custom scheme             Tauri IPC
                   (GET, files, /api/)       (bodies, socket, open server)
                 +---------v----------------------v--------------+
                 |  mobile shell (mobile/src)                    |
                 |    one window, one webview, one server active |
                 |  shared host core (experimental/app/src)      |
                 |    web_bundle  web_trust  web_proxy  web_tls  |
                 |    web_vault   web_ws     web_json local_route|
                 +----------------------|------------------------+
                                        | HTTPS (rustls, OS verifier)
                                        v
                              the member's Mnema server
```

The desktop host is one Rust crate of about 8,000 lines. Its core is already
free of window code and compiles for any target: the bundle store and its
verification (`web_bundle`), the trust statement (`web_trust`), the proxy with
the cookie jar and limits (`web_proxy`), the TLS stack (`web_tls`), the vault
abstraction (`web_vault`), the WebSocket relay (`web_ws`), the JSON limits
(`web_json`) and the closed list of app routes (`local_route`). The mobile
crate includes these files by `#[path]`, the same way the desktop host already
includes the platform-neutral gaming logic from the root crate. The shared test
vectors in `test-vectors/` therefore keep covering the mobile build with no
extra work: it is the same code.

What the mobile shell does itself, because the desktop code is built around
several windows and webviews (Tauri's `unstable` multi-webview, which is
desktop-only):

- **One webview.** The page for adding a server and the page of the server are
  the same webview; the host knows which role is active and serves the bundle
  accordingly. The identity the web app reads (`__MNEMA_TRUSTED_APP__`) is
  written by the host for the active role, so the web app needs no change: it
  already accepts `https://mnema.localhost` (the Windows form, which Android
  uses too) and `mnema://localhost` (the macOS form, which iOS uses too).
- **Commands by role, not by label.** The desktop host picks the server by the
  label of the calling webview. On the phone there is one webview, so every
  command checks the active role and the app origin of the page instead.
- **One server at a time at first.** The desktop client separates the browser
  storage of each server (a WebView2 profile, a macOS data store). The web app
  keeps the device key of the end-to-end media in that storage, so two servers
  must never share it. The phone starts like macOS before 14: one server, its
  storage wiped when the member switches. Several servers side by side need a
  per-server data store on the phone too (`WKWebsiteDataStore(forIdentifier:)`
  on iOS 17 and newer, the WebView `Profile` API of AndroidX WebKit on Android),
  which Tauri does not expose yet. That is a Tauri plugin of our own or an
  upstream change; see the phases.

What cannot be shared, and why:

| Area | Desktop | Mobile |
| --- | --- | --- |
| Window, rail, menus, dialogs | Tauri multi-webview, native menus | Not available on mobile; a phone-sized server list as a host page |
| Session at rest | Keychain, Credential Manager (`keyring-core`) | iOS Keychain (the `apple-native-keyring-store` protected store) and Android Keystore; until then the 0600 file in the app's private data directory, which another app cannot read |
| TLS trust | `rustls-platform-verifier` on CryptoAPI / Security.framework | Same crate, Security.framework on iOS; on Android it calls the system trust manager through JNI and needs its Kotlin component in the APK and an init call at start (done in the spike) |
| Request bodies | Delivered by the webview | **Android WebView hands the host no request body** for a custom scheme (`shouldInterceptRequest` has none). Every POST, PUT and PATCH to `/api/` goes over Tauri IPC instead; see below |
| Updates of the app | Signed self-update (minisign) | The stores update the app; a sideloaded APK is updated by its installer or by F-Droid |
| Updates of the bundle | Downloaded from mnema-talk releases, verified | Same code; see the App Store note on downloaded code |
| Notifications | Native toasts while running | Local notifications while running (`tauri-plugin-notification`); background push is native work |
| Gaming | Windows only | None |

**The body bridge (Android).** Android's WebView gives the host the URL, the
method and the headers of a request to the app's own scheme, but never its
body. A small page shim, installed as an initialization script like the
desktop's `buffered-bodies.js`, sends requests with a body to the host as one
IPC call (`mobile_api`: method, path, the allowed headers, the bytes). The host
answers through the same proxy function the custom scheme uses, so the rules
for headers, caps, cookies and redirects stay in one place. iOS's
`WKURLSchemeHandler` delivers bodies, so the bridge is only needed there for
streamed bodies, which the web app does not send.

## Alternatives compared

| | Tauri 2 mobile (recommended) | Capacitor | PWA only | Native shells (Kotlin, Swift) |
| --- | --- | --- | --- | --- |
| Signed bundle, server supplies no code | Yes, same Rust code as desktop | Only with a port of verification and proxy to Kotlin and Swift (or to JavaScript inside the page it is meant to protect) | No: the server serves the JavaScript | Yes, but written twice more |
| Session out of the page | Yes (host cookie jar) | Port needed | No (browser cookie) | Yes, written twice |
| Shared test vectors | Same code | Ports must be held to them | Not applicable | Two more ports |
| Push when closed | Native work (below) | Plugins exist (FCM/APNs only) | **Works today**: Web Push in Chrome on Android, and on iOS 16.4+ for a web app added to the home screen | Native work |
| Store presence | Yes | Yes | No (Android can wrap a PWA as a Trusted Web Activity; it then runs the server's code) | Yes |
| Toolchain cost | Rust, Android NDK, Xcode; CI already runs Rust | Node, Gradle, Xcode | None | Gradle and Xcode, two codebases |
| Maturity on mobile | Tauri mobile is young; one window, fewer plugins | Mature | Mature | Mature |

The PWA path is not a weaker copy: on Android it already has reliable push
through Chrome, and on iOS through the home screen web app. What it cannot give
is the trust model of the desktop client (the server can change the JavaScript
at any time) and a store listing. The recommendation is therefore to keep the
PWA as the recommended phone path and build the Tauri app as the trusted
client, like the desktop client is for computers. Capacitor would force the
trust core into a second and third language; native shells would do the same
and also duplicate everything else. React Native or Flutter would mean a second
user interface and are not considered.

## Day one and native work

What works as soon as the app starts (the shared core, no new native code):

- The server picker of the verified bundle (the same page as the desktop's
  "add a server"), address checks, `/api/health` discovery and bundle
  selection by `web_client_api` and server version.
- The verified bundle at the app origin, its CSP and the closed route list.
- `/api/` GET requests through the proxy with the host's cookie jar.
- Messages, channels, threads, search, settings: everything the web app does
  over GET plus the body bridge, and the live socket through `desktop_ws_*`.
- Microphone and camera permission prompts: wry forwards the WebView's
  permission requests to the Android runtime permissions; iOS asks through
  `NSMicrophoneUsageDescription` and `NSCameraUsageDescription`.
- The phone layout of the web app (drawer, full-screen thread, safe areas,
  44 px targets), which the e2e projects already cover in device emulation.

What needs native work:

| Item | Android | iOS |
| --- | --- | --- |
| Request bodies | The body bridge (IPC) | Not needed |
| Session in the keystore | Android Keystore key wrapping the jar file | Keychain item (`keyring-core` protected store) |
| Background push | UnifiedPush (and optionally FCM through a gateway) | APNs, which needs the owner's Apple account; see push |
| Calls in the background | A foreground service of type `microphone` (Android 14 needs the type and its permission), audio focus, ConnectionService for the system call UI | `UIBackgroundModes` audio and voip, an `AVAudioSession` in play-and-record, CallKit for the system call UI |
| Screen sharing from the phone | Not possible in a WebView: the WebView has no `getDisplayMedia`; MediaProjection would need a native WebRTC sender | Not possible: iOS has no `getDisplayMedia`; ReplayKit broadcast extensions run outside the WebView |
| Viewing a screen share | Works (WebRTC in the WebView) | Works |
| Invite links | Custom scheme intent filter, share target | Custom URL scheme, share extension |
| Several servers | Per-server WebView profile | Per-server `WKWebsiteDataStore` (iOS 17+) |
| Badge | Launcher badge through notifications | App icon badge |
| Files | Downloads to the system file picker; uploads already work from the WebView file chooser | Same, through the share sheet |

Sealed (end-to-end encrypted) media needs `RTCRtpScriptTransform`, Ed25519 in
WebCrypto and module workers. Android WebView follows Chrome and has them on
current versions; WKWebView on iOS 17 and newer has them as Safari does. Older
systems are refused at the join by the web app itself, as in a browser, so the
app needs no extra rule. The minimum versions follow from that: Android 7.0
(API 24, the Tauri default) for the app, a current Android System WebView for
calls, and iOS 17 for calls.

## Push notifications

The server already sends Web Push (RFC 8030), encrypted for the receiver
(RFC 8291, `aes128gcm`) and signed with the server's own VAPID key (RFC 8292),
to any HTTPS push endpoint on a public address
(`internal/push/validate.go`). The payload is finished, localised text plus ids
(`internal/push/payload.go`), and the push service sees only ciphertext.
Subscriptions are registered with `POST /api/push/subscriptions`.

A WebView cannot receive Web Push: there is no push-capable service worker in
Android WebView or WKWebView. So the app needs a native receiver, and the
question is how a self-hosted server reaches it **without a central service of
ours**.

**Android: UnifiedPush, with no server change.** UnifiedPush distributors (ntfy,
NextPush, a self-hosted ntfy, or the embedded FCM distributor) hand the app an
HTTPS endpoint that accepts Web Push requests. The app generates the P-256 key
pair and auth secret itself, registers the endpoint with the server's existing
subscription API exactly as a browser would, and decrypts the payload on the
device (the UnifiedPush Android connector does RFC 8291). The server's push
code, its VAPID key, its endpoint check and its privacy settings stay as they
are; the distributor sees ciphertext. A member without a distributor installed
gets no background push and is told so in the app's settings.

**Android with FCM.** Firebase Cloud Messaging delivers only for the Firebase
project the app was built with, and only to senders holding that project's
service account. Two honest options:

1. *The operator's own Firebase project*, only for an operator who builds and
   signs their own APK (the source is public): the server would need an FCM
   sender next to Web Push. Not for the official app, because an official app
   has one project.
2. *A gateway run by whoever publishes the app*, the UnifiedPush "embedded FCM
   distributor" model: the server sends its usual encrypted Web Push to the
   gateway, the gateway wraps the opaque bytes into an FCM message for the
   app's project. The gateway is stateless and sees only ciphertext, but it is
   a service someone must run. For the official app that someone is the
   owner, which contradicts "no central service of ours" unless it stays
   optional.

**iOS: APNs only.** Apple delivers background notifications only through
APNs, and only to a sender holding a key of the Apple developer account that
published the app. A self-hosted server cannot hold that key. The established
pattern (used by Mastodon and Matrix clients) is a relay: the server sends its
ordinary encrypted Web Push to a relay URL, the relay forwards the opaque
payload to APNs, and a Notification Service Extension in the app decrypts it on
the device. The relay keeps no state, logs no content and cannot read the
payload. Without it, the iOS app gets notifications only while it is open.

**The relay design that stays true to self-hosting.**

- The push path is always Web Push from the server; nothing in the server knows
  about FCM or APNs. A relay is just another Web Push endpoint, which the
  server already accepts.
- The relay is a small open-source program in this project (a few hundred
  lines: accept a Web Push request, check its size and VAPID header, forward to
  APNs or FCM, answer). Anyone can run it; an operator who builds their own app
  points it at their own Apple or Firebase account.
- The official app's relay is the owner's decision: run one (it is needed for
  iOS push and for FCM on Android), or ship the official iOS app without
  background push and the Android app with UnifiedPush only. The app lets a
  member pick the relay URL, so nobody is tied to the owner's.
- Privacy is unchanged from the browser: the payload is encrypted for the
  device, the relay and Apple or Google see a sender, a size and a time. The
  existing setting for previews (`PushPreviews`) still decides whether the
  text is in the payload at all.

## Calls and audio

Calls run in the WebView with the web app's own WebRTC and sealed media, as in
the browser. On day one a call works while the app is in the foreground. What
the platforms do when the app leaves the screen:

- **Android** stops the microphone of a backgrounded app and may stop its
  process. A foreground service of type `microphone` (with the
  `FOREGROUND_SERVICE_MICROPHONE` permission on Android 14 and newer) and an
  ongoing notification keep the call alive; the web app tells the host that a
  Talk is connected (the desktop already has `desktop_gaming_sync` for that
  state, which the mobile host can reuse as a plain "in a call" signal).
  ConnectionService adds the system call UI and Bluetooth routing, later.
- **iOS** suspends a WKWebView in the background unless the app declares the
  `audio` background mode and holds an active `AVAudioSession` in
  play-and-record. CallKit gives the system call UI, the lock screen and the
  audio route; an incoming call while the app is closed needs a VoIP push
  (PushKit), which Apple requires to be reported to CallKit at once. That is a
  later phase and depends on the APNs relay.
- **Audio routing** (earpiece, speaker, Bluetooth) is set by the platform audio
  session; the web app's output device picker cannot change it inside a
  WebView. The host sets speaker for video calls and the default route
  otherwise.
- **Noise suppression** is the web app's own (the only AI-type feature the
  product has) and runs in the WebView unchanged.
- **Screen sharing** from a phone is not offered (see the table above).
  Viewing a share works.

## Invite links and deep links

An invite link is `https://<server>/invite/<code>` (the code is 6 to 64
letters, digits, `_` and `-`, `local_route.rs`). The server picker of the app
already accepts a pasted invite link and opens the join screen.

Verified **https app links** (Android App Links, iOS Universal Links) do not fit
self-hosting: both need every domain written into the app at build time and a
file served by that domain (`assetlinks.json`, `apple-app-site-association`).
The official app cannot list the domains of every community. So:

- **A custom scheme** `mnema://invite?server=<origin>&code=<code>` (and
  `mnema://open?server=<origin>`) opens the app at the picker with the server
  and code filled in; the member confirms before anything is added, as on the
  desktop ("Add this server"). The host validates both parts with the same
  `address` and `invite_segment` code as the desktop. `tauri-plugin-deep-link`
  registers the scheme on both platforms. The scheme name `mnema` is the same
  word as the app's internal origin on iOS but a separate system (the URL
  scheme is the operating system's, the WebView scheme handler is per webview);
  if that proves confusing, `mnema-talk` is the fallback name.
- **The server's invite page** shows "Open in the app" with that link when it
  sees a phone; the browser path stays the default.
- **Share to the app**: sharing a link from any app into Mnema (Android
  `ACTION_SEND`, an iOS share extension later) fills the picker.
- An operator who builds their own app can add verified app links for their
  own domain; the server can serve the two `.well-known` files from
  configuration then. Not needed for the official app.

## Store requirements

**Apple App Store.**

- An Apple Developer Program membership (99 USD a year), as an individual or an
  organisation (an organisation needs a D-U-N-S number). The name on the
  account is shown as the seller.
- **TestFlight** for testing: internal testers right away, external testers
  after a short beta review.
- Review guideline 1.2 (user-generated content) requires a way to filter
  objectionable material, to report content, to block abusive users, and
  published contact information. Mnema has: reporting a message
  (`POST /api/messages/{id}/report`, `ReportDialog.vue`) with a moderation queue
  for admins (`/api/admin/reports`), blocking a member (`/api/me/blocks`),
  automod rules (`/api/admin/automod/rules`), timeouts, kicks and bans. The
  contact is each community's operator, which the review notes must explain:
  the app connects to servers the publisher does not run. Reporting **a user**
  (not only a message) is worth adding before review.
- Guideline 5.1.1(v): in-app account deletion. Exists
  (`POST /api/users/me/delete` from the settings). A data export exists too
  (`/api/users/me/export`).
- Guideline 2.1: the reviewer needs a working server and a demo account. The
  owner must run a public demo community for review, with a demo account kept
  alive during review.
- Guideline 4.2 (minimum functionality): an app that only wraps a website is
  refused. The case for this app: the verified bundle, the session in the
  Keychain, push, CallKit and several servers. Push should be in the first
  submission for that reason.
- Guideline 2.5.2 (no downloaded code that changes the app): the bundle update
  downloads signed JavaScript. Web content run by WebKit is generally allowed,
  but a reviewer may ask. The fallback is to ship iOS builds that use only the
  built-in bundle and update it with app updates; the code supports both.
- Privacy: a privacy policy URL, the App Privacy answers (the app itself
  collects nothing; each server's data is the operator's), and the privacy
  manifest (`PrivacyInfo.xcprivacy`) for the required-reason APIs.
- Export compliance: the app uses standard cryptography (TLS, the web app's
  sealed media). The owner answers the encryption questions in App Store
  Connect; standard encryption is usually exempt, but the answer is the
  owner's.
- In the EU, the Digital Services Act trader status: a trader's address, phone
  number and e-mail are published on the store page.

**Google Play.**

- A Play Console developer account (25 USD once), with identity verification.
  New personal accounts must run a **closed test with at least 12 testers for
  14 days** before they may publish to production.
- The **data safety form**: what the app collects and shares. The app sends
  data only to the server the member chose; the form must still declare
  messages, photos, audio and the account as "collected" because they leave
  the device, with "encrypted in transit".
- The target API level Google requires at the time of submission (it rises
  every year), the Play app signing key held by Google with an upload key held
  by the owner, and the same user-generated content rules as Apple (report,
  block, a policy).
- Foreground service declarations (the call service) with a short video for
  review.

**F-Droid and sideloading.**

- The F-Droid main repository builds from source and allows no proprietary
  libraries: no FCM, no prebuilt binaries. UnifiedPush fits. The Rust and NDK
  build is possible on F-Droid's build servers but slow to get accepted, and
  the `rustls-platform-verifier` Kotlin component, which the spike takes as a
  prebuilt AAR, would have to be built from its source there.
- **Own F-Droid repository**: the project can publish a signed F-Droid repo
  next to the GitHub releases (an index signed with a repo key). Members add
  it once in F-Droid and get updates. Much less effort than the main
  repository.
- **Sideloaded signed APKs** on GitHub releases work today on most devices.
  Google has announced that Android on certified devices will require apps,
  sideloaded ones included, to come from a verified developer, starting in a
  few countries in 2026 and wider from 2027. Check the state at release time;
  it may require the same developer verification as Play.

## Signing and what the owner must decide

The owner's standing decision is that the desktop client ships without OS code
signing (no notarization, no Authenticode). That stays. Mobile is different:
**no store accepts an unsigned app, and Android installs nothing unsigned**.
The spike signs only with a throwaway debug key generated in CI, which is fine
for testing and useless for distribution (a new key per run means no updates).

Decisions only the owner can make:

1. **Whether to publish to the stores at all**, or Android only, or sideloaded
   and F-Droid only.
2. **Apple**: enrol in the Apple Developer Program (individual or
   organisation), accept that the name is public, and choose whether the EU
   trader status applies.
3. **Google**: open a Play Console account, verify identity, find 12 testers
   for the closed test.
4. **Push**: run the push relay for the official app (needed for iOS push and
   for FCM), or ship iOS without background push and Android with UnifiedPush
   only.
5. **The demo community** for store review, public and kept running.
6. **The app identifiers**, which can never change after the first release:
   proposed `com.mnema.talk` (the desktop uses `com.mnema.desktop.shareddev`).
7. **Privacy policy and support URL** for the store pages.

What the owner must do once decided (no agent can, because each needs the
owner's identity or money):

- Enrol at Apple, create the app in App Store Connect, create an APNs key, and
  either add a CI secret for an App Store Connect API key or sign on a Mac.
- Create the Play Console app, generate the upload key (kept offline, with a
  backup), and add it as an encrypted CI secret.
- For sideloaded APKs and an own F-Droid repo: generate a release signing key
  and keep it offline; losing it means members must reinstall.

Agents can do everything else: the code, the CI, the store texts, the privacy
answers as a draft, the relay, the screenshots from CI simulators.

## CI

`.github/workflows/mobile.yml` (spike, not a required check):

- **Android** on `ubuntu-latest`: Java 17, Android SDK command-line tools with
  pinned `platforms`, `build-tools` and NDK versions, the pinned Rust
  toolchain with the Android targets, the pinned Tauri CLI from `ui/`. It
  stages the signed web bundle as the desktop jobs do, builds a debug APK for
  `aarch64` and `x86_64`, signs it with a debug key that exists only in that
  run, and uploads it as an artifact. A second job boots an x86_64 emulator
  without camera, microphone or audio, installs the APK, starts a stand-in
  server (`scripts/fake-instance.mjs`) reachable through `adb reverse`, and
  reads the app's self-test lines and a screenshot.
- **iOS** on `macos-15`: the same toolchain with the iOS simulator target,
  `xcodebuild` for the simulator with code signing off, the `.app` uploaded as
  an artifact, then the same self-test in a booted simulator.
- Actions pinned by commit SHA, like the rest of the repository; no secrets.

Later: release jobs that sign with the owner's keys from encrypted secrets,
upload to TestFlight and the Play internal track, and publish the APK and an
F-Droid index to the GitHub release.

## Phases and estimates

Estimates are working days of focused development with agents, after the
owner's decisions they depend on. Store review times come on top.

| Phase | Content | Estimate | Needs from the owner |
| --- | --- | --- | --- |
| 0. Spike | This document, the mobile crate, CI builds and the self-test | Done (see below) | Nothing |
| 1. Android alpha | Body bridge, socket, session in the Keystore, one server, local notifications while open, invite custom scheme, CI self-test against a real server | 5 to 8 days | Nothing (debug-signed APKs as CI artifacts) |
| 2. iOS alpha | Keychain session, simulator CI against a real server, safe areas and keyboard checks | 4 to 6 days | Apple account for TestFlight |
| 3. Push | UnifiedPush on Android; the relay program; APNs with a Notification Service Extension | 8 to 12 days | Relay decision, APNs key |
| 4. Calls in the background | Android foreground service, iOS audio session; then ConnectionService and CallKit | 6 to 10 days | Nothing (CallKit for incoming calls needs phase 3) |
| 5. Several servers | Per-server data stores (a Tauri plugin), a phone-sized server list | 5 to 8 days | Nothing |
| 6. Store submission | Store texts, screenshots from CI, privacy forms, demo community, Play closed test (14 days) | 3 to 5 days plus review | Accounts, demo community, testers |

Roughly two to three months of calendar time to both stores, most of it
waiting for accounts, tests and review. Android can be useful as a sideloaded
alpha after phase 1.

## Spike: what was proven

The spike lives in [`mobile/`](../mobile): a Tauri 2 app crate that includes
the shared core of `experimental/app/src` by path and adds a mobile shell (one
webview, role-based commands, the Android TLS init) with the Android and iOS
projects generated by the Tauri CLI. The desktop crate is not changed.

State on 2026-11-13 (workflow `Mobile spike`, all three jobs green):

| Step | Android (API 34 emulator, x86_64) | iOS (simulator, macOS 15 runner) |
| --- | --- | --- |
| Build | Debug APK for aarch64 and x86_64, debug key of that run, as artifact | Unsigned simulator `.app` (aarch64), as artifact |
| Lint | `clippy -D warnings` for `aarch64-linux-android`, with and without the self-test | `clippy -D warnings` for `aarch64-apple-ios-sim`, both ways |
| App starts, verified bundle served at the app origin | Yes (`https://mnema.localhost`) | Yes (`mnema://localhost`) |
| The server picker of the verified bundle on screen | Yes (screenshot: "Connect to your community", "Verified app v3.0.12, built into the installer") | Yes, the same |
| TLS through the shared client with the system's certificate check | Yes: `https://github.com/` answered, so the JNI init and the Kotlin half work | Yes |
| Server opened through the picker's code path (address, `/api/health`, bundle selection) | Yes | Yes |
| The server's page ran from the bundle and reached the server through the proxy | Yes: the stand-in server saw `GET /api/auth/me` and `/api/auth/options` with the origin the host sets, and the page showed the server's sign-in | Yes, the same |

The self-test opens the server itself (`mobile-selftest`, a build-time
address) instead of typing into the picker, because CI does not drive the
screen. The screenshots and the step log are artifacts of each run. The
emulator is not fully steady: in one run Android killed the app because
Google Play services died inside the emulator ("depends on provider
com.google.android.gms ... in dying proc"), and a re-run passed. A
`default` system image without Play services is the next thing to try there.

**Not proven yet** (phase 1 work, in this order):

1. **Signing in end to end.** The Android body bridge (`mobile_api`,
   `page-shim/ipc-bodies.js`) is written for `fetch`, but no POST has gone
   through it: the self-test sends only GET requests.
2. **Uploads.** The upload with progress uses `XMLHttpRequest`
   (`shared/lib/xhrUpload.ts`), which the bridge does not cover yet.
3. **The live socket** (`desktop_ws_*` through `web_ws`) against a real
   server; the stand-in only answers GET.
4. **The session in the Keystore and the Keychain.** The cookie jar rests in a
   private 0600 file in the app's data directory today.
5. **The app icon and name.** Both are still the Tauri defaults
   ("Mnema Talk Preview", `com.mnema.talk.preview`).
6. **The white area on iOS** below the page (the safe-area background).
7. **A real server in CI** instead of the stand-in (`scripts/fake-instance.mjs`),
   as the desktop jobs already run one.
8. **A steadier emulator**: a `default` system image without Google Play
   services, after one run in which Play services died and took the app with
   it.

Open owner decisions that the later phases wait for are in
[Signing and what the owner must decide](#signing-and-what-the-owner-must-decide):
above all the push relay (iOS push and FCM) and the Apple and Google
developer accounts.
