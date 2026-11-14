# Changelog

What changed for the people who use the desktop client. Releases up to 1.3.5
are described on the [releases page](https://github.com/msch128/mnema-talk-desktop/releases).

## 1.4.0 (not released yet)

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
