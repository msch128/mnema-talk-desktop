/**
 * What the rail needs from the desktop beyond its own page: a native menu and a
 * small dialog. Both live above everything else (the menu is the system's, the
 * dialog a window of its own), so a server's page in front of the rail can never
 * clip them. The rail page describes them (`core/railui.ts` checks what it
 * sends); the answer goes back to the rail page.
 */
import { BrowserWindow, Menu, ipcMain } from 'electron'
import { TITLE } from '../core/title.ts'
import { APP_URL } from '../core/origin.ts'
import { DIALOG_WIDTH, type DialogResult, type DialogSpec, type MenuItem, type MenuSpec, cancelled, checkDialogResult, dialogHeight, menuIds } from '../core/railui.ts'
import { PARTITIONS, type Context } from './context.ts'
import { mainFrameUrl } from './handle.ts'
import { dialogPreload, harden, pagePreferences } from './windows.ts'

// ------------------------------------------------------------------- menu

/** The menu the person can still answer: only its token and its ids are accepted back. */
const open = new WeakMap<Context, { token: string, ids: Set<string> }>()

function template(items: readonly MenuItem[], chosen: (id: string) => void): Electron.MenuItemConstructorOptions[] {
  return items.map((item): Electron.MenuItemConstructorOptions => {
    if (item.separator || item.id === undefined) return { type: 'separator' }
    const { id } = item
    // An `&` marks a mnemonic in a native label; the name's own is doubled.
    const label = process.platform === 'darwin' ? (item.text ?? id) : (item.text ?? id).replaceAll('&', '&&')
    const enabled = item.disabled !== true
    if (item.items) return { label, enabled, submenu: template(item.items, chosen) }
    if (item.checked !== undefined) return { label, enabled, type: 'checkbox', checked: item.checked, click: () => chosen(id) }
    return { label, enabled, click: () => chosen(id) }
  })
}

/** Tells the rail page which entry was picked (nothing happens when the menu is dismissed). */
function chose(ctx: Context, spec: MenuSpec, id: string): void {
  const current = open.get(ctx)
  const rail = ctx.shell.rail
  if (!current || current.token !== spec.token || !current.ids.has(id) || !rail || rail.webContents.isDestroyed()) return
  void rail.webContents.executeJavaScript(`window.mnemaRail&&window.mnemaRail.chosen(${JSON.stringify(spec.token)},${JSON.stringify(id)})`).catch(() => undefined)
}

/**
 * Shows the menu as the system's own popup. `x` and `y` (logical pixels from the
 * top left of the window; the rail stands at the origin) place it for the
 * keyboard; a negative one opens it at the pointer.
 */
export function showMenu(ctx: Context, spec: MenuSpec, x: number, y: number): void {
  open.set(ctx, { token: spec.token, ids: menuIds(spec.items) })
  const menu = Menu.buildFromTemplate(template(spec.items, (id) => chose(ctx, spec, id)))
  const window = ctx.shell.window
  const at = x >= 0 && y >= 0 ? { x: Math.round(x), y: Math.round(y) } : {}
  menu.popup({ ...(window && !window.isDestroyed() ? { window } : {}), ...at })
}

// ----------------------------------------------------------------- dialog

interface Open {
  window: BrowserWindow
  spec: DialogSpec
  finish(result: DialogResult): void
}
const dialogs = new WeakMap<Context, Open>()

/**
 * Opens the dialog and waits for the answer. One at a time: a second request
 * while one is open only brings it forward and counts as cancelled.
 */
export function showDialog(ctx: Context, spec: DialogSpec): Promise<DialogResult> {
  const existing = dialogs.get(ctx)
  if (existing && !existing.window.isDestroyed()) {
    existing.window.focus()
    return Promise.resolve(cancelled(spec))
  }
  const parent = ctx.shell.window && !ctx.shell.window.isDestroyed() ? ctx.shell.window : undefined
  return new Promise((resolve) => {
    const window = new BrowserWindow({
      title: TITLE,
      width: DIALOG_WIDTH,
      height: dialogHeight(spec),
      useContentSize: true,
      resizable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      show: false,
      autoHideMenuBar: true,
      ...(parent ? { parent, modal: true } : {}),
      webPreferences: pagePreferences(PARTITIONS.dialog, dialogPreload()),
    })
    window.removeMenu()
    window.on('page-title-updated', (event) => event.preventDefault())
    harden(ctx, window.webContents, 'dialog', () => window)
    let settled = false
    const finish = (result: DialogResult): void => {
      if (settled) return
      settled = true
      dialogs.delete(ctx)
      resolve(result)
      if (!window.isDestroyed()) window.close()
    }
    dialogs.set(ctx, { window, spec, finish })
    window.once('closed', () => finish(cancelled(spec)))
    window.once('ready-to-show', () => window.show())
    void window.loadURL(`${APP_URL}dialog.html`)
  })
}

/** The dialog window asks for its spec and answers; nobody else is heard. */
export function registerDialogIpc(ctx: Context): void {
  const own = (event: Electron.IpcMainEvent): Open | null => {
    const current = dialogs.get(ctx)
    if (!current || current.window.isDestroyed() || event.sender.id !== current.window.webContents.id || mainFrameUrl(event) === null) return null
    return current
  }
  ipcMain.on('mnema:dialog-spec', (event) => {
    event.returnValue = JSON.stringify(own(event)?.spec ?? null)
  })
  ipcMain.on('mnema:dialog-done', (event, answer: unknown) => {
    const current = own(event)
    if (current) current.finish(checkDialogResult(current.spec, answer))
  })
}
