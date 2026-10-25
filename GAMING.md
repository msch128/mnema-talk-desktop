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
| macOS, Linux | none: the macOS and Linux desktop clients are paused, and the gaming integration is Windows only for now. Detection is off and the settings say so. | - |

The foreground source sits behind the `ForegroundSource` trait
(`src/foreground.rs`) with a fake for tests. The operating-system code is in
`src/foreground_windows.rs`.

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
   | Steam | `steamapps/libraryfolders.vdf` and `steamapps/appmanifest_*.acf` of every library | Steam tools and runtimes (Steamworks Shared, SteamVR, Proton, Linux runtimes, Wallpaper Engine) are skipped |
   | Epic Games Store | `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item` | add-ons and unfinished installs are skipped |
   | GOG Galaxy | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\*` (`gameName`, `path`) | |
   | EA app | `HKLM\SOFTWARE\WOW6432Node\EA Games\*` and `HKLM\SOFTWARE\EA Games\*` (`Install Dir`) | |
   | Battle.net | not read | its install list is a binary database without a public format; Battle.net games are found by the bundled list or a name you add |

   Every path read from a manifest or the registry is checked before the disk
   is touched: only absolute local drive paths are opened. UNC (`\\host\share`),
   device (`\\?\`, `\\.\`) and relative paths, `..` segments and alternate
   data streams are skipped silently, so a crafted manifest cannot make Windows
   contact another machine. A manifest folder that is a drive root, a system folder or a broad folder such
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

### The bundled list

`src/game-list.json` is maintained by hand. It holds about 600 well-known PC
games from the last quarter century (shooters, battle royales, MOBAs, MMOs,
survival, sports, racing, simulators, strategy and indie hits): a display name,
one or more executable file names and, for some, a Steam app id kept for
reference. It is deliberately not complete and has not been checked against
installed copies. The launcher libraries (above) cover the rest: any game your
Steam, Epic, GOG or EA app reports is found there without being in the list,
and you can add a name yourself in the gaming settings. A missing or wrong name
only means that game is found through its launcher or a name you add instead.
A unit test keeps names, executables and ids unique and well formed. Corrections
and verified names are welcome under the repository licence.

### Sources

The list is our own work, written from public knowledge of the executable names
games ship. No third-party list, database, API response or website was copied,
scraped or derived, and nothing from Discord's detectable-applications list
(its Developer Policy forbids reuse) is used. An earlier catalog that came from
such a list was deleted; the Git history still contains that old file (names and
executable names only), and it is kept as is because rewriting public history
would break signed release tags and forks. No file in the current tree carries
data derived from it. The header of `src/game-list.json` states the provenance.
