# Gaming integration: how it works and what it never does

The gaming integration (sidepeek, overlay, global shortcuts, push-to-talk) is
Beta and Windows-first. This page is the reference for the desktop client. The
user-facing setup guide is in the main repository
([desktop-app.md](https://github.com/msch128/mnema-talk/blob/main/doc/desktop-app.md#windows-gaming-dev)).

## Anti-cheat safety statement

Online games run anti-cheat software, and a chat client must never look like a
cheat. This is what the client does, and what it will never do.

It does:

- Ask the operating system to tell it when the focused application changes
  (a documented, passive notification), and look up the image path of that one
  process.
- Read read-only launcher manifests on disk (for example the list of installed
  Steam games) to learn which folders are game installs.
- Draw its own windows (the sidepeek and the overlay) as ordinary topmost
  windows of its own process.

It never:

- Enumerates processes or windows to find games. Detection starts from the
  focus-change notification and touches only the process that is in front.
- Injects code or a DLL into any other process, hooks another process, reads or
  writes another process's memory, or opens a handle with more access than
  "query limited information" to ask for its image path.
- Installs a driver or a kernel component.
- Hooks a game's graphics API. There is no in-game rendering; the overlay is a
  separate window, which is why it cannot draw over exclusive fullscreen.
- Sends synthetic input to a game or reads the game's keyboard or mouse state
  beyond the push-to-talk and shortcut keys the user configured.
- Sends the image path, window title or process name of what you run to any
  server. A matched game name stays on the device.

## Game detection

Detection is event driven. There is no timer that looks at the foreground
window.

| System | Source | Looks up |
| --- | --- | --- |
| Windows | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT)` on a dedicated thread with its own message loop (out of context: nothing is loaded into other processes) | `QueryFullProcessImageNameW` on the foreground window's single PID, opened with `PROCESS_QUERY_LIMITED_INFORMATION` |
| macOS | `NSWorkspaceDidActivateApplicationNotification` | pid and executable path of the activated application; no permission prompt |
| Linux (Electron client) | none: the Linux client has no gaming integration | - |
| Wayland | none: the protocol gives applications no way to see which window is focused, by design. Detection is off and the settings say so. | - |

The foreground source sits behind the `ForegroundSource` trait
(`src/foreground.rs`) with a fake for tests. The operating-system code is in
`src/foreground_windows.rs` and `src/foreground_macos.rs`.

A window counts as a game only when it was announced by the source and is still
the foreground window when the sidepeek or shortcuts act, so an announcement
that has been overtaken by a newer focus change never shows a game.

## Which game is it?

Three sources, checked in this order. The first match wins and decides the name.

1. **Names you added** in the gaming settings (an executable file name such as
   `MyGame.exe`).
2. **Launcher libraries.** The install folders of the games your launchers
   report. A program that lives under one of those folders is that game, and the
   game is named as the launcher names it. Read-only, from files and registry
   keys the launchers keep for themselves:

   | Launcher | Read | Notes |
   | --- | --- | --- |
   | Steam (Windows, macOS) | `steamapps/libraryfolders.vdf` and `steamapps/appmanifest_*.acf` of every library | Steam tools and runtimes (Steamworks Shared, SteamVR, Proton, Linux runtimes, Wallpaper Engine) are skipped |
   | Epic Games Store (Windows) | `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item` | add-ons and unfinished installs are skipped |
   | GOG Galaxy (Windows) | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\*` (`gameName`, `path`) | |
   | EA app (Windows) | `HKLM\SOFTWARE\WOW6432Node\EA Games\*` and `HKLM\SOFTWARE\EA Games\*` (`Install Dir`) | |
   | Battle.net | not read | its install list is a binary database without a public format; Battle.net games are found by the bundled list or a name you add |

   A manifest folder that is a drive root, a system folder or a broad folder such
   as `Program Files` itself is ignored, so one bad manifest cannot make every
   program a game. Manifests larger than 256 KiB are ignored.
3. **The bundled list** (`src/game-list.json`): executable file names of widely
   played PC games. Matching is by file name only.

The launcher scan runs on its own thread: once at start, every 15 minutes, and
whenever the gaming settings are saved. The foreground path only reads the last
finished result. Java is never matched by name: `javaw.exe` counts as Minecraft
only when its window title says so, and a Java program inside a launcher's game
folder is that game. Crash handlers, uninstallers and redistributables inside a
game folder are not the game.

### The bundled list and Discord's list

The list is our own work. Discord publishes a list of detectable games, but its
Developer Policy forbids copying it, so nothing from it is used or derived: the
file names were written by hand by the maintainers from their own knowledge of
what games ship, and no third-party list, database or website was copied. The
header of `src/game-list.json` states this. The list is smaller than a crowd
sourced one (a few hundred titles) and has not been checked against installed
copies; a missing or wrong name only means that game is found through its
launcher library or a name you add instead. Contributions of verified names are
welcome under the repository licence.

## Push-to-talk and shortcuts

Keys are event driven on every system. Nothing asks "is this key down?" in a
loop.

| System | Source | Notes |
| --- | --- | --- |
| Windows | Raw Input (`RegisterRawInputDevices` with `RIDEV_INPUTSINK`, keyboard and mouse) on its own thread with a hidden window | No keyboard hook, no `GetAsyncKeyState`. Works while a game has the focus. A session lock clears all keys, so a key cannot stay stuck down across the lock screen. |
| macOS | A listen-only `CGEventTap` | Needs Input Monitoring; see below. Listen-only: it can observe, never change or delay an event. |
| Linux | none | The Linux client has no gaming integration; its shortcuts work while the window is in front. |
| Wayland | none | The compositor does not let applications see keys outside their window. The GlobalShortcuts portal is not used because it reports presses without a dependable release, which push-to-talk cannot work with; push-to-talk works while Mnema is in front. |

What is read: for each change only the key (or the mouse side button 4 or 5)
and whether it went down or up. Text is never looked at, no key history is kept
and nothing about keystrokes is logged or sent. The host keeps the set of keys
currently down and compares it with your four chords (overlay, mute, deafen,
push-to-talk); only a change in whether a chord is down wakes the rest of the
app. A press that begins and ends between two looks is still delivered once.

The source runs only while you are in a Talk with gaming on (on macOS: with
push-to-talk on), and stops when you leave. On Windows the host asks the
windowing library to release the process's own raw input registration first,
because Windows allows one registration per process; if something else still
holds it, the settings say so and push-to-talk works while Mnema is in front.

Push-to-talk and the other shortcuts act only while a detected game is in the
foreground, as before. The web page accepts a `ptt-press` for 150 ms and drops
the press 300 ms after the last one (a safeguard against a stuck microphone), so
while the key is held the host repeats the press every 100 ms. That is a
keepalive of a state known from the key event, not input polling. Leaving the
game, leaving the Talk, a session lock or a stalled host releases push-to-talk.

### macOS Input Monitoring

The permission is requested only when you turn push-to-talk on in a Talk, never
at start. Mnema then shows its own explanation with an "Open System Settings"
button (and macOS shows its one-time prompt); "Not now" leaves everything else
working and asks nothing further until the next start. Once you allow Mnema in
System Settings > Privacy & Security > Input Monitoring the host starts the tap
on its next beat; if macOS still refuses it, restart Mnema. Without the
permission the app works as before and push-to-talk works while Mnema is in
front. macOS draws no overlay or sidepeek: it only gets shortcuts and
push-to-talk during a detected game.

### Known limits

- Raw Input does not see the secure desktop (UAC prompts, Ctrl+Alt+Del). A key
  released there is not reported; the session-lock reset and the page's
  300 ms safeguard bound the damage.
- Games that read the keyboard exclusively (some old DirectInput exclusive
  modes) do not change what Raw Input delivers, but we cannot test every title.
- Not verified on real hardware or against any anti-cheat; see the safety
  statement above for what the client does and never does.
