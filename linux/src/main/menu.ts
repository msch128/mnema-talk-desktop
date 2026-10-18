import { Menu, app } from 'electron'
import { menuText } from '../core/selfupdate.ts'
import type { Context } from './context.ts'
import { openUpdater } from './windows.ts'

/** The window menu: updates, the usual editing commands, zoom and full screen. No reload and no developer tools. */
export function buildMenu(ctx: Context): void {
  const phase = ctx.updater?.snapshot().phase ?? 'idle'
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    {
      label: 'Mnema Desktop',
      submenu: [
        { id: 'native-update', label: menuText(phase), click: () => menuClicked(ctx) },
        { type: 'separator' },
        { label: 'Quit', accelerator: 'CmdOrCtrl+Q', click: () => app.quit() },
      ],
    },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    { label: 'View', submenu: [{ role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }, { type: 'separator' }, { role: 'togglefullscreen' }] },
  ]))
}

/** Menu entry: show the window and look again. */
export function menuClicked(ctx: Context): void {
  openUpdater(ctx, true)
  const updater = ctx.updater
  if (updater && !updater.isBusyOrWaiting()) void updater.check(true)
}
