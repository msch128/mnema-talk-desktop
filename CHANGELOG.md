# Changelog

What changed for the people who use the desktop client. Releases up to 1.3.5
are described on the [releases page](https://github.com/msch128/mnema-talk-desktop/releases).

## 1.4.0

### Any number of servers

- There is no server limit. The rail scrolls, Ctrl+1 to Ctrl+8 reach the first
  eight servers and Ctrl+9 the last one, and an import of thousands of servers
  is taken at once (the list file can be up to 8 MiB).
- A server's page is loaded when you open it, and a page hidden for a while is
  closed again to save memory (the + menu: Unload idle servers, off, after 10
  minutes or after 1 hour). A page with an unsent draft, a file waiting to be
  sent or a Talk is never closed.
- For a server without a loaded page the client keeps the unread dot and the
  mention count itself, including direct messages, and a quiet notification
  says when its mentions rise. The 64 servers used most recently stay
  connected; the others are checked every minute.

### Update now, everywhere

- A pill at the foot of the server rail shows a waiting update and its
  progress, and an **Updates** menu has **Update now**, **Check for updates…**
  and the installed version. During a Talk the client asks first and can
  install after the call.
- The Windows portable ZIP updates itself too and keeps its `mnema-data`
  folder.

### Updates from the new address

- The client asks for updates at `github.com/mnema-xyz/talk-desktop` first and
  at the old address second, so it keeps finding them after the repository
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
