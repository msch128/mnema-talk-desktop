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
