import { Menu, app } from 'electron'
import { menuText } from '../core/selfupdate.ts'
import { serverMenuAction } from '../core/notify.ts'
import type { Context } from './context.ts'
import { activateNth, startAdding } from './shell.ts'
import { openUpdater } from './windows.ts'

/** Ctrl+1 to Ctrl+9 reach the servers, named as the rail names them; Ctrl+Shift+N adds one. */
function serversMenu(ctx: Context): Electron.MenuItemConstructorOptions {
  const slots = ctx.trusted.registry.ids().map((id) => ctx.trusted.registry.slot(id))
  const items: Electron.MenuItemConstructorOptions[] = Array.from({ length: 9 }, (_, index) => {
    const slot = slots[index]
    return { id: `server-${index + 1}`, label: slot ? `${index + 1} \u2014 ${ctx.trusted.registry.displayName(slot)}` : `Server ${index + 1}`, accelerator: `CmdOrCtrl+${index + 1}`, enabled: slot !== undefined, click: () => serverMenuClicked(ctx, `server-${index + 1}`) }
  })
  items.push({ type: 'separator' }, { id: 'server-add', label: 'Add server\u2026', accelerator: 'CmdOrCtrl+Shift+N', click: () => serverMenuClicked(ctx, 'server-add') })
  return { label: 'Servers', submenu: items }
}

export function serverMenuClicked(ctx: Context, id: string): void {
  const action = serverMenuAction(id)
  if (action?.kind === 'add') startAdding(ctx)
  else if (action) activateNth(ctx, action.number)
}

/** The window menu: servers, updates, the usual editing commands, zoom and full screen. No reload and no developer tools. */
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
    serversMenu(ctx),
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
