import { Menu, app } from 'electron'
import { menuView } from '../core/selfupdate.ts'
import { serverMenuAction } from '../core/notify.ts'
import { menuEntryText, shortcutPlace } from '../core/instances.ts'
import type { Context } from './context.ts'
import { quickSwitch } from './shell.ts'
import { activateNth, startAdding } from './shelllayout.ts'
import { menuGerman, updateNow } from './updater.ts'
import { openUpdater } from './windows.ts'

/** Ctrl+1 to Ctrl+8 reach the first eight servers and Ctrl+9 the last, named as the rail names them; Ctrl+Shift+N adds one. */
function serversMenu(ctx: Context): Electron.MenuItemConstructorOptions {
  const slots = ctx.trusted.registry.ids().map((id) => ctx.trusted.registry.slot(id))
  const items: Electron.MenuItemConstructorOptions[] = Array.from({ length: 9 }, (_, index) => {
    const number = index + 1
    const place = shortcutPlace(number, slots.length)
    const slot = place === null ? undefined : slots[place]
    return { id: `server-${number}`, label: menuEntryText(number, slot ? ctx.trusted.registry.displayName(slot) : null), accelerator: `CmdOrCtrl+${number}`, enabled: slot !== undefined, click: () => serverMenuClicked(ctx, `server-${number}`) }
  })
  items.push({ type: 'separator' }, { id: 'server-add', label: 'Add server\u2026', accelerator: 'CmdOrCtrl+Shift+N', click: () => serverMenuClicked(ctx, 'server-add') }, { id: 'server-switch', label: 'Quick switcher\u2026', accelerator: 'CmdOrCtrl+K', click: () => serverMenuClicked(ctx, 'server-switch') })
  return { label: 'Servers', submenu: items }
}

export function serverMenuClicked(ctx: Context, id: string): void {
  const action = serverMenuAction(id)
  if (action?.kind === 'add') startAdding(ctx)
  else if (action?.kind === 'switch') quickSwitch(ctx)
  else if (action) activateNth(ctx, action.number)
}

/** The update entries, as in the Tauri host: "Update now", "Check for updates" and the installed version; a dot in the title marks a waiting update. */
function updateMenu(ctx: Context): Electron.MenuItemConstructorOptions {
  const snapshot = ctx.updater?.snapshot()
  const view = snapshot ? menuView(snapshot, menuGerman()) : null
  if (!view) return { label: 'Updates', enabled: false }
  return {
    label: view.title,
    submenu: [
      { id: 'native-update-now', label: view.now, enabled: view.nowEnabled, click: () => { updateNow(ctx, false) } },
      { id: 'native-update', label: view.check, click: () => menuClicked(ctx) },
      { type: 'separator' },
      { label: view.version, enabled: false },
    ],
  }
}

/** The window menu: servers, updates, the usual editing commands, zoom and full screen. No reload and no developer tools. */
export function buildMenu(ctx: Context): void {
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    {
      label: 'Mnema Desktop',
      submenu: [
        updateMenu(ctx),
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
