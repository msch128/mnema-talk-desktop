/**
 * The one window around the servers: a rail of the host on the left, and the
 * page of one server (or the page for adding one) next to it. A port of
 * `experimental/app/src/web_shell.rs`.
 *
 * Every server has a view of its own in a session of its own (a persistent
 * partition: its storage is its own and survives a restart), and its own
 * session in the registry. A page learns nothing of the others: the handler of
 * its session and every command pick the server by that session, never by an
 * argument. The rail is a page of the host, independent of any bundle and of
 * any server; it is the only one that can switch, reorder or remove a server.
 */
import { existsSync, readdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { BaseWindow, WebContentsView, app } from 'electron'
import { parseAddress } from '../core/client.ts'
import { AddError, MAX_INSTANCES, idFor } from '../core/instances.ts'
import { APP_URL } from '../core/origin.ts'
import { LEGACY_PARTITION, PARTITIONS, attachmentPartition, serverPartition, type Context } from './context.ts'
import { buildMenu } from './menu.ts'
import { setLauncherBadge, toastsClosed } from './notify.ts'
import { prepare, prepareServer } from './sessions.ts'
import { appHandler, assetHandler, HOST_PAGE_CSP } from './protocol.ts'
import { loadAssets } from './assets.ts'
import { appPreload, harden, pagePreferences, railPreload } from './windows.ts'

export const RAIL_WIDTH = 68
const TITLE = 'Mnema Desktop DEV'

// ------------------------------------------------------------------- layout

function frontKey(ctx: Context): string | null {
  const registry = ctx.trusted.registry
  const active = registry.active()
  return active !== null && !registry.adding() ? active : null
}

/** Puts the rail on the left, the page in front next to it and hides the others. Hidden pages keep running, the engine only stops painting them. */
export function layout(ctx: Context): void {
  const { window, rail, selector, pages } = ctx.shell
  if (!window || window.isDestroyed()) return
  const { width, height } = window.getContentBounds()
  rail?.setBounds({ x: 0, y: 0, width: RAIL_WIDTH, height })
  const content = { x: RAIL_WIDTH, y: 0, width: Math.max(1, width - RAIL_WIDTH), height }
  const front = frontKey(ctx)
  if (selector) { selector.setBounds(content); selector.setVisible(front === null) }
  for (const [id, view] of pages) { view.setBounds(content); view.setVisible(id === front) }
}

export function focusFront(ctx: Context): void {
  const front = frontKey(ctx)
  const view = front === null ? ctx.shell.selector : ctx.shell.pages.get(front)
  if (view && !view.webContents.isDestroyed()) view.webContents.focus()
}

/** The host can show another server next to the first (storage of each is kept apart by partitions on every Linux). */
function canAdd(): boolean {
  return true
}

export function railSnapshot(ctx: Context) {
  return ctx.trusted.registry.rail(canAdd())
}

/** Shows the rail what it should draw, and renames the menu entries to match. */
export function pushRail(ctx: Context): void {
  const state = railSnapshot(ctx)
  const rail = ctx.shell.rail
  if (rail && !rail.webContents.isDestroyed()) {
    void rail.webContents.executeJavaScript(`window.mnemaRail&&window.mnemaRail.set(${JSON.stringify(state)})`).catch(() => undefined)
  }
  buildMenu(ctx)
}

/** The counters changed: the launcher badge is their sum, the rail shows each. */
export function countsChanged(ctx: Context): void {
  setLauncherBadge(ctx.trusted.registry.badge())
  pushRail(ctx)
}

function changed(ctx: Context): void {
  ctx.trusted.saveInstances()
  layout(ctx)
  pushRail(ctx)
}

/** Brings a server to the front (a toast was used, the person chose it) and the window with it. */
export function bringForward(ctx: Context, id: string): void {
  if (!ctx.trusted.registry.activate(id)) return
  changed(ctx)
  const window = ctx.shell.window
  if (window && !window.isDestroyed()) {
    if (window.isMinimized()) window.restore()
    window.show()
    window.focus()
  }
  focusFront(ctx)
}

export function activate(ctx: Context, id: string): void {
  if (ctx.trusted.registry.activate(id)) { changed(ctx); focusFront(ctx) }
}

export function activateNth(ctx: Context, number: number): void {
  if (ctx.trusted.registry.activateNth(number) !== null) { changed(ctx); focusFront(ctx) }
}

export function startAdding(ctx: Context): void {
  if (ctx.trusted.registry.startAdding()) { changed(ctx); focusFront(ctx) }
}

export function cancelAdding(ctx: Context): void {
  ctx.trusted.registry.stopAdding()
  changed(ctx)
  focusFront(ctx)
}

export function moveServer(ctx: Context, id: string, to: number): void {
  ctx.trusted.registry.moveTo(id, to)
  changed(ctx)
}

// ------------------------------------------------------------------- windows

function hostView(ctx: Context, partition: string, preload: string | null): WebContentsView {
  const view = new WebContentsView({ webPreferences: pagePreferences(partition, preload) })
  view.webContents.on('page-title-updated', (event) => event.preventDefault())
  return view
}

/** Opens the window with the rail and the page for adding a server, and starts the servers of the list. */
export function createShell(ctx: Context, assets: (name: string) => ReturnType<typeof loadAssets>): void {
  const window = new BaseWindow({ title: TITLE, width: 1200, height: 820, minWidth: 640, minHeight: 480, autoHideMenuBar: false })
  ctx.shell.window = window
  prepare(ctx, PARTITIONS.rail, 'rail', assetHandler('/', assets('rail-ui'), HOST_PAGE_CSP))
  prepare(ctx, PARTITIONS.selector, 'selector', appHandler('selector', ctx.trusted, ctx.debug))

  const rail = hostView(ctx, PARTITIONS.rail, railPreload())
  harden(ctx, rail.webContents, 'rail', () => window)
  const selector = hostView(ctx, PARTITIONS.selector, appPreload())
  harden(ctx, selector.webContents, 'selector', () => window)
  ctx.shell.rail = rail
  ctx.shell.selector = selector
  window.contentView.addChildView(rail)
  window.contentView.addChildView(selector)
  void rail.webContents.loadURL(APP_URL)
  void selector.webContents.loadURL(APP_URL)

  window.on('resize', () => layout(ctx))
  window.on('focus', () => focusFront(ctx))
  window.on('closed', () => shutDown(ctx))
  sweepOrphans(ctx)
  for (const id of ctx.trusted.registry.ids()) void startServer(ctx, id)
  layout(ctx)
  pushRail(ctx)
  window.show()
}

/** The window is gone: every connection ends and the program with it. */
function shutDown(ctx: Context): void {
  for (const id of ctx.trusted.registry.ids()) ctx.trusted.registry.session(id)?.sockets.closeAll()
  ctx.shell.window = null
  app.quit()
}

/** Creates the page of a running server, in a session of its own. */
function openPage(ctx: Context, id: string): void {
  if (ctx.shell.pages.has(id) || !ctx.shell.window) return
  const partition = prepareServer(ctx, id)
  const view = hostView(ctx, partition, appPreload())
  ctx.servers.set(view.webContents.id, id)
  harden(ctx, view.webContents, 'instance', () => ctx.shell.window ?? undefined)
  view.webContents.once('destroyed', () => { if (ctx.shell.pages.get(id) === view) ctx.shell.pages.delete(id) })
  ctx.shell.pages.set(id, view)
  view.setVisible(false)
  ctx.shell.window.contentView.addChildView(view)
  void view.webContents.loadURL(APP_URL)
}

// ------------------------------------------------------------------- servers

/**
 * Checks a server and starts its page: the server is asked what it is, the
 * verified bundle for it is chosen (never its own JavaScript), and the page
 * opens. A server that cannot be served keeps its place in the list and says
 * why in its own tab.
 */
export async function startServer(ctx: Context, id: string): Promise<void> {
  const registry = ctx.trusted.registry
  const slot = registry.slot(id)
  if (!slot) return
  slot.phase = { kind: 'starting' }
  pushRail(ctx)
  try {
    const session = await ctx.trusted.connect(slot.origin)
    const current = registry.slot(id)
    if (!current) { session.sockets.closeAll(); return }
    registry.restart(id)
    current.phase = { kind: 'ready', session }
  } catch (error) {
    const current = registry.slot(id)
    if (current) current.phase = { kind: 'refused', why: error instanceof Error ? error.message.slice(0, 300) : 'The server could not be started.' }
  }
  const page = ctx.shell.pages.get(id)
  if (registry.session(id)) {
    if (page) void page.webContents.loadURL(APP_URL)
    else openPage(ctx, id)
  } else if (page) {
    void page.webContents.loadURL(APP_URL)
  }
  layout(ctx)
  pushRail(ctx)
}

/** The page for adding a server asks to open one: only then does a server join the list. */
export async function addServer(ctx: Context, address: string): Promise<void> {
  const registry = ctx.trusted.registry
  const origin = parseAddress(address)
  const id = idFor(origin)
  if (registry.contains(id)) { bringForward(ctx, id); return }
  if (registry.length >= MAX_INSTANCES) throw new AddError('full')
  const session = await ctx.trusted.connect(origin)
  try {
    registry.add(origin)
  } catch (error) {
    session.sockets.closeAll()
    if (error instanceof AddError && error.kind === 'known') { bringForward(ctx, id); return }
    throw error
  }
  const slot = registry.slot(id)
  if (slot) { registry.restart(id); slot.phase = { kind: 'ready', session } }
  // The first server after the update keeps the storage of the single-server client (its end-to-end state).
  if (ctx.shell.adoptLegacy && registry.length === 1) { registry.legacy = id; ctx.shell.adoptLegacy = false }
  registry.activate(id)
  try {
    openPage(ctx, id)
  } catch {
    await removeServer(ctx, id)
    throw new Error('Unable to open the server.')
  }
  changed(ctx)
  focusFront(ctx)
}

export async function reconnect(ctx: Context, id: string): Promise<void> {
  if (ctx.trusted.registry.slot(id) && !ctx.trusted.registry.session(id)) await startServer(ctx, id)
}

/** The bundle set changed (a key was revoked, a cap withdrew one, the first bundle arrived): pages move to what is held now. */
export function bundlesChanged(ctx: Context): void {
  const selector = ctx.shell.selector
  if (selector && !selector.webContents.isDestroyed()) void selector.webContents.loadURL(APP_URL)
  for (const id of ctx.trusted.registry.ids()) {
    if (ctx.trusted.registry.session(id)) continue
    void startServer(ctx, id)
  }
  pushRail(ctx)
}

// ----------------------------------------------------------------- removal

function partitionDir(ctx: Context, partition: string): string {
  return join(ctx.trusted.dataDirectory() ?? app.getPath('userData'), 'Partitions', partition.replace(/^persist:/, ''))
}

/**
 * Removes what a server's page stored in the browser engine, and then its
 * partition directory. The engine may hold the files a moment longer, so this
 * tries again for a while, in the background, and gives up if the server comes
 * back in the meantime.
 */
function wipeDirectory(ctx: Context, id: string, dir: string): void {
  let attempts = 0
  const attempt = () => {
    if (ctx.trusted.registry.contains(id) || !existsSync(dir)) return
    try {
      rmSync(dir, { recursive: true, force: true })
    } catch { /* tried again below */ }
    if (existsSync(dir) && ++attempts < 20) setTimeout(attempt, 500).unref()
  }
  attempt()
}

/**
 * Takes a server out of the list and wipes everything this device holds of it:
 * its stored session (secret store entry and fallback file), its browser
 * storage, its sockets, its toasts and its attachment window.
 */
export async function removeServer(ctx: Context, id: string): Promise<void> {
  const { trusted } = ctx
  const partition = serverPartition(ctx, id)
  const slot = trusted.registry.remove(id)
  if (!slot) return
  if (slot.phase.kind === 'ready') {
    slot.phase.session.sockets.closeAll()
    slot.phase.session.instance.jar.forget()
  } else {
    trusted.forgetStoredSession(slot.origin)
  }
  toastsClosed(id, slot.generation)
  const attachment = ctx.shell.attachments.get(id)
  const stores = [attachment?.webContents.session]
  if (attachment && !attachment.isDestroyed()) attachment.close()
  const page = ctx.shell.pages.get(id)
  if (page) {
    stores.push(page.webContents.session)
    ctx.shell.pages.delete(id)
    ctx.shell.window?.contentView.removeChildView(page)
    page.webContents.close()
  }
  for (const ses of stores) {
    if (!ses) continue
    await ses.clearStorageData().catch(() => undefined)
    await ses.clearCache().catch(() => undefined)
  }
  wipeDirectory(ctx, id, partitionDir(ctx, partition))
  wipeDirectory(ctx, id, partitionDir(ctx, attachmentPartition(id)))
  changed(ctx)
  countsChanged(ctx)
  focusFront(ctx)
}

/** Storage of servers that are no longer in the list (a removal cut short by the end of the program) goes at the next start. */
function sweepOrphans(ctx: Context): void {
  const root = join(ctx.trusted.dataDirectory() ?? app.getPath('userData'), 'Partitions')
  let names: string[]
  try { names = readdirSync(root) } catch { return }
  const known = new Set(ctx.trusted.registry.ids())
  for (const name of names) {
    const match = /^(?:inst|mnema-att)-([0-9a-f]{16})$/.exec(name)
    if (match && !known.has(match[1] as string)) rmSync(join(root, name), { recursive: true, force: true })
  }
}

/** Whether the single-server client left its storage behind and no list of servers exists yet. */
export function legacyStorageWaits(ctx: Context): boolean {
  const dir = partitionDir(ctx, LEGACY_PARTITION)
  return existsSync(dir) && ctx.trusted.registry.length === 0
}
