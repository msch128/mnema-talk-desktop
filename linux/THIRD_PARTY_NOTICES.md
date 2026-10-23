# Linux client (Electron) — third-party software

The Linux client in [linux/](.) ships Electron with the Chromium and Node.js
that it embeds, one JavaScript library, and the signed Mnema Talk web app.
This file is installed next to the program (`resources/THIRD_PARTY_NOTICES.md`),
together with the project's `LICENSE`; the licences of Electron and Chromium
are in the program directory (`LICENSE.electron.txt`, `LICENSES.chromium.html`).
The Windows and macOS clients are a different program (the Tauri host); their
inventory is [../THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

## What is shipped

| Software | Version | Licence | Where |
|---|---|---|---|
| [Electron](https://www.electronjs.org/) | 45.0.0 | MIT | the program (`mnema-desktop-dev`) and its libraries |
| [Chromium](https://www.chromium.org/) | embedded in Electron | BSD-3-Clause and the licences of its own dependencies | `LICENSES.chromium.html` lists every one |
| [Node.js](https://nodejs.org/) | embedded in Electron | MIT and the licences of its own dependencies | `LICENSES.chromium.html` |
| [ws](https://github.com/websockets/ws) | 8.22.0 | MIT | `resources/app.asar` (`node_modules/ws`), text below |
| Mnema Talk web app | the signed bundle's version | AGPL-3.0, with its own notices | `resources/app.asar` (`bundle/builtin.zip`); the notices of its dependencies are in the app (account menu) and in [mnema-talk](https://github.com/msch128/mnema-talk/blob/main/THIRD_PARTY_NOTICES.md) |

The AppImage also carries the AppImage runtime (MIT, [AppImage/type2-runtime](https://github.com/AppImage/type2-runtime)),
and the update window and the screen picker use the same colour palette as the
rest of the project but no third-party font files or images.

## Build-only tools

They are not part of the program and are recorded in
[package-lock.json](package-lock.json): electron-builder (MIT),
@electron/fuses (MIT), TypeScript (Apache-2.0), ESLint and typescript-eslint
(MIT), the type definitions of Node.js and ws (MIT), globals (MIT), jiti (MIT).
electron-builder fetches the AppImage and `.deb` tooling (fpm, squashfs, the
AppImage runtime) while packaging; none of it is in the package except the
runtime.

## ws

```text
Copyright (c) 2011 Einar Otto Stangvik <einaros@gmail.com>
Copyright (c) 2013 Arnout Kazemier and contributors
Copyright (c) 2016 Luigi Pinca and contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```
