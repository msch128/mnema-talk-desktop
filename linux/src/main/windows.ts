/**
 * Windows. Every window is context isolated, sandboxed and without Node; its
 * navigation and new windows are locked down to the app origin; HTTPS links
 * leave only after a confirmation, through the system browser.
 */
import { join } from 'node:path'
import { BrowserWindow, Menu, app, dialog, shell, type WebContents } from 'electron'
import { APP_URL, appApi, bundled, parseUrl, pickerUi, updateUi } from '../core/origin.ts'
import { externalAllowed } from '../core/client.ts'
import { PARTITIONS, type Context, type Role } from './context.ts'

const TITLE = 'Mnema Desktop DEV'

function preloadPath(): string {
  return join(import.meta.dirname, '..', 'preload', 'preload.cjs')
}

/** Developer tools are off in a release; a development run may turn them on. */
function devTools(): boolean {
  return !app.isPackaged && process.env.MNEMA_DESKTOP_DEVTOOLS === '1'
}

function allowed(role: Role, url: URL): boolean {
  switch (role) {
    case 'selector': return bundled(url)
    // Routes of the app, and the API paths that downloads and attachment links navigate to. The proxy answers the latter.
    case 'instance': return bundled(url) || appApi(url)
    case 'attachment': return appApi(url)
    case 'updater': return updateUi(url)
    case 'picker': return pickerUi(url)
  }
}

/** Asks, then opens an HTTPS link in the system browser. */
export async function openExternal(parent: BrowserWindow | undefined, url: URL): Promise<void> {
  if (!externalAllowed(url)) return
  const options = { type: 'question' as const, buttons: ['Open', 'Cancel'], defaultId: 1, cancelId: 1, title: TITLE, message: 'Open this link in your browser?', detail: url.href, noLink: true }
  const { response } = parent && !parent.isDestroyed() ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options)
  if (response === 0) await shell.openExternal(url.href).catch(() => undefined)
}

function contextMenu(contents: WebContents): void {
  contents.on('context-menu', (_event, params) => {
    const items: Electron.MenuItemConstructorOptions[] = []
    if (params.isEditable) {
      items.push({ role: 'cut', enabled: params.editFlags.canCut }, { role: 'copy', enabled: params.editFlags.canCopy }, { role: 'paste', enabled: params.editFlags.canPaste }, { type: 'separator' }, { role: 'selectAll' })
    } else if (params.selectionText.trim() !== '') {
      items.push({ role: 'copy' })
    }
    if (items.length > 0) Menu.buildFromTemplate(items).popup()
  })
}

function harden(ctx: Context, win: BrowserWindow, role: Role): void {
  const contents = win.webContents
  ctx.roles.set(contents.id, role)
  const check = (event: Electron.Event, raw: string) => {
    const url = parseUrl(raw)
    if (url && allowed(role, url)) return
    event.preventDefault()
    if (url && role === 'instance') void openExternal(win, url)
  }
  contents.on('will-navigate', (event, url) => check(event, url))
  contents.on('will-redirect', (event, url) => check(event, url))
  contents.on('will-frame-navigate', (event) => { if (!event.isMainFrame) event.preventDefault() })
  contents.on('will-attach-webview', (event) => event.preventDefault())
  contents.setWindowOpenHandler(({ url }) => {
    const target = parseUrl(url)
    if (role === 'instance' && target) {
      if (appApi(target)) openAttachment(ctx, target)
      else void openExternal(win, target)
    }
    return { action: 'deny' }
  })
  contents.on('render-process-gone', (_event, details) => {
    console.error(`Mnema Desktop: the ${role} window's renderer ended: ${details.reason}`)
  })
  contents.once('destroyed', () => ctx.roles.delete(contents.id))
  contextMenu(contents)
}

interface Options {
  role: Role
  title: string
  width: number
  height: number
  url: string
  /** Brought to the front; false shows the window without taking focus. */
  focus?: boolean
  fixed?: boolean
  parent?: BrowserWindow
  /** No preload: the window gets no identity and no commands. */
  bare?: boolean
}

function open(ctx: Context, options: Options): BrowserWindow {
  const win = new BrowserWindow({
    title: options.title,
    width: options.width,
    height: options.height,
    minWidth: options.fixed ? undefined : 480,
    minHeight: options.fixed ? undefined : 420,
    resizable: !options.fixed,
    maximizable: !options.fixed,
    show: false,
    parent: options.parent,
    modal: options.parent !== undefined,
    autoHideMenuBar: true,
    webPreferences: {
      partition: PARTITIONS[options.role],
      ...(options.bare ? {} : { preload: preloadPath() }),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
      experimentalFeatures: false,
      navigateOnDragDrop: false,
      safeDialogs: true,
      devTools: devTools(),
      spellcheck: false,
    },
  })
  // The title is the host's, not whatever the page says.
  win.on('page-title-updated', (event) => event.preventDefault())
  ctx.windows[options.role] = win
  harden(ctx, win, options.role)
  win.once('closed', () => { if (ctx.windows[options.role] === win) delete ctx.windows[options.role] })
  win.once('ready-to-show', () => { if (options.focus === false) win.showInactive(); else win.show() })
  void win.loadURL(options.url)
  return win
}

/** The instance selector: the verified app in its selector role. */
export function openSelector(ctx: Context): BrowserWindow {
  const win = open(ctx, { role: 'selector', title: `${TITLE} — Instance`, width: 520, height: 520, url: APP_URL })
  return win
}

/** The window of the selected instance. */
export function openInstance(ctx: Context, onClosed: () => void): BrowserWindow {
  const win = open(ctx, { role: 'instance', title: TITLE, width: 1200, height: 820, url: APP_URL })
  win.once('closed', onClosed)
  return win
}

/** Downloads and attachments open here: the proxy's answers, forced into a download or inline-safe media. */
export function openAttachment(ctx: Context, url: URL): void {
  const existing = ctx.windows.attachment
  if (existing && !existing.isDestroyed()) {
    void existing.loadURL(url.href)
    existing.focus()
    return
  }
  open(ctx, { role: 'attachment', title: `${TITLE} — Attachment`, width: 900, height: 700, url: url.href, bare: true })
}

export function openUpdater(ctx: Context, focus: boolean): BrowserWindow {
  const existing = ctx.windows.updater
  if (existing && !existing.isDestroyed()) {
    if (focus) { existing.show(); existing.focus() }
    return existing
  }
  return open(ctx, { role: 'updater', title: `${TITLE} — Update`, width: 440, height: 340, url: `${APP_URL}__update/index.html`, fixed: true, focus })
}

export function openPicker(ctx: Context, parent: BrowserWindow | undefined): BrowserWindow {
  return open(ctx, { role: 'picker', title: `${TITLE} — Share your screen`, width: 760, height: 540, url: `${APP_URL}__picker/index.html`, parent, fixed: true })
}
