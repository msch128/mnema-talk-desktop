# Changelog

What changed for the people who use the desktop client. Releases up to 1.3.5
are described on the [releases page](https://github.com/msch128/mnema-talk-desktop/releases).

## 1.4.0

### Any number of servers

One client now holds any number of servers in the rail: there is no server
limit. The rail scrolls when your servers do not fit, and `Ctrl+9` (`Cmd+9` on
macOS) jumps to the last server.

- Servers in the background sleep and wake when needed. Only the server in
  front loads at start; a page hidden for more than 10 minutes is closed while
  more than four run, unless it holds an unsent draft, an upload or a Talk.
  Opening the server loads its page again.
- While a server sleeps, the client keeps its unread dot and direct message
  counts on the rail and the badge itself, and announces new mentions in a
  quiet notification.
- The **+** menu sets when idle servers unload (**Unload idle servers**: Off,
  after 10 minutes, after 1 hour).

### Update now

When a newer version is available, a calm **Update** pill appears at the foot
of the server rail, and the app menu gets an **Updates** entry (**Update now**,
**Check for updates…**, and the installed version; on macOS under the
application menu). No window opens by itself.

- **Update now** (from the pill or the menu) downloads with progress in the
  pill, verifies the signature, installs and restarts the app. Your servers,
  sign-ins and settings stay.
- During a Talk the app asks first and offers **After the call**: the update
  downloads and verifies right away, then installs once your call ends.
- If an update fails, the app says what went wrong and what to do next, with a
  retry. See [Updates](README.md#updates).

### The portable ZIP updates itself

On Windows, the portable copy now updates itself: it downloads the signed
portable ZIP of the new release, verifies it with the same pinned key, puts the
new program in place and restarts.

- The `mnema-data` folder stays, so your servers, sign-ins and settings are
  kept.
- The folder containing the portable copy must be writable.

### New update address

The client now asks `github.com/mnema-xyz/talk-desktop` for updates first and
the old `msch128` address second, so updates keep coming after the repository
moves.

### Renamed: Mnema Desktop

The app is now called **Mnema Desktop**: the window titles, the installers, the
Start menu and desktop shortcuts, the entry under Installed apps, the macOS app
and the Linux menu entry. Up to 1.3.5 it was **Mnema Desktop DEV**.

- An installed copy updates in place, and your servers, sessions and settings
  stay. On Windows the old program, its entry and its shortcuts are replaced
  by the new ones; a shortcut pinned to the taskbar has to be pinned again. On
  macOS an updated app keeps its old name on disk. On Linux the `.deb` keeps
  its package name (`mnema-desktop-dev`) and its command, and moves to
  `/opt/Mnema Desktop`. See [From "Mnema Desktop DEV"](README.md#from-mnema-desktop-dev).
- The release files are named `Mnema-Desktop_<version>_…` instead of
  `Mnema-Desktop-DEV_<version>_…`. The unversioned links under
  [Get it](README.md#get-it) stay the same.
