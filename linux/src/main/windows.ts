/**
 * Windows. Every window is context isolated, sandboxed and without Node; its
 * navigation and new windows are locked down to the app origin; HTTPS links
 * leave only after a confirmation, through the system browser.
 */
import { join } from 'node:path'
import { BaseWindow, BrowserWindow, Menu, app, dialog, shell, type WebContents } from 'electron'
import { prepareAttachment } from './sessions.ts'
import { APP_URL, appApi, bundled, parseUrl, pickerUi, updateUi } from '../core/origin.ts'
import { externalAllowed } from '../core/client.ts'
import { PARTITIONS, attachmentPartition, type Context, type Role } from './context.ts'

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
    case 'rail': return bundled(url)
    case 'selector': return bundled(url)
    // Routes of the app, and the API paths that downloads and attachment links navigate to. The proxy answers the latter.
    case 'instance': return bundled(url) || appApi(url)
    case 'attachment': return appApi(url)
    case 'updater': return updateUi(url)
    case 'picker': return pickerUi(url)
  }
}

/** Asks, then opens an HTTPS link in the system browser. */
export async function openExternal(parent: BaseWindow | undefined, url: URL): Promise<void> {
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

/** Locks a page down: navigation and new windows stay inside the app, links leave only through the browser after a confirmation. */
export function harden(ctx: Context, contents: WebContents, role: Role, parent: () => BaseWindow | undefined): void {
  ctx.roles.set(contents.id, role)
  const check = (event: Electron.Event, raw: string) => {
    const url = parseUrl(raw)
    if (url && allowed(role, url)) return
    event.preventDefault()
    if (url && role === 'instance') void openExternal(parent(), url)
  }
  contents.on('will-navigate', (event, url) => check(event, url))
  contents.on('will-redirect', (event, url) => check(event, url))
  contents.on('will-frame-navigate', (event) => { if (!event.isMainFrame) event.preventDefault() })
  contents.on('will-attach-webview', (event) => event.preventDefault())
  contents.setWindowOpenHandler(({ url }) => {
    const target = parseUrl(url)
    const server = ctx.servers.get(contents.id)
    if (role === 'instance' && target && server) {
      if (appApi(target)) openAttachment(ctx, server, target)
      else void openExternal(parent(), target)
    }
    return { action: 'deny' }
  })
  contents.on('render-process-gone', (_event, details) => {
    console.error(`Mnema Desktop: the ${role} page's renderer ended: ${details.reason}`)
  })
  const id = contents.id
  contents.once('destroyed', () => { ctx.roles.delete(id); ctx.servers.delete(id) })
  contextMenu(contents)
}

/** What every page of the client is: isolated, sandboxed, without Node. */
export function pagePreferences(partition: string, preload: string | null): Electron.WebPreferences {
  return {
    partition,
    ...(preload ? { preload } : {}),
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
  }
}

export function appPreload(): string {
  return preloadPath()
}

export function railPreload(): string {
  return join(import.meta.dirname, '..', 'preload', 'rail.cjs')
}

interface Options {
  role: 'updater' | 'picker' | 'attachment'
  title: string
  width: number
  height: number
  url: string
  partition: string
  /** Brought to the front; false shows the window without taking focus. */
  focus?: boolean
  fixed?: boolean
  parent?: BaseWindow
  /** The server an attachment window belongs to. */
  server?: string
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
    webPreferences: pagePreferences(options.partition, options.bare ? null : preloadPath()),
  })
  // The title is the host's, not whatever the page says.
  win.on('page-title-updated', (event) => event.preventDefault())
  if (options.server) ctx.servers.set(win.webContents.id, options.server)
  harden(ctx, win.webContents, options.role, () => win)
  if (options.role === 'attachment') {
    if (options.server) ctx.shell.attachments.set(options.server, win)
    const server = options.server
    win.once('closed', () => { if (server && ctx.shell.attachments.get(server) === win) ctx.shell.attachments.delete(server) })
  } else {
    ctx.windows[options.role] = win
    win.once('closed', () => { if (ctx.windows[options.role as 'updater' | 'picker'] === win) delete ctx.windows[options.role as 'updater' | 'picker'] })
  }
  win.once('ready-to-show', () => { if (options.focus === false) win.showInactive(); else win.show() })
  void win.loadURL(options.url)
  return win
}

/** Downloads and attachments of a server open here: the proxy's answers, forced into a download or inline-safe media. */
export function openAttachment(ctx: Context, server: string, url: URL): void {
  const existing = ctx.shell.attachments.get(server)
  if (existing && !existing.isDestroyed()) {
    void existing.loadURL(url.href)
    existing.focus()
    return
  }
  prepareAttachment(ctx, server)
  open(ctx, { role: 'attachment', title: `${TITLE} — Attachment`, width: 900, height: 700, url: url.href, partition: attachmentPartition(server), server, bare: true })
}

export function openUpdater(ctx: Context, focus: boolean): BrowserWindow {
  const existing = ctx.windows.updater
  if (existing && !existing.isDestroyed()) {
    if (focus) { existing.show(); existing.focus() }
    return existing
  }
  return open(ctx, { role: 'updater', title: `${TITLE} — Update`, width: 440, height: 340, url: `${APP_URL}__update/index.html`, partition: PARTITIONS.updater, fixed: true, focus })
}

export function openPicker(ctx: Context, parent: BaseWindow | undefined): BrowserWindow {
  return open(ctx, { role: 'picker', title: `${TITLE} — Share your screen`, width: 760, height: 540, url: `${APP_URL}__picker/index.html`, partition: PARTITIONS.picker, parent, fixed: true })
}
