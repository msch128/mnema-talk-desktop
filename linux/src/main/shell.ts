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
import { randomBytes } from 'node:crypto'
import { existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { BaseWindow, WebContentsView, app, session } from 'electron'
import { inviteCode, parseAddress } from '../core/client.ts'
import { AddError, MAX_INSTANCES, idFor } from '../core/instances.ts'
import type { Edit } from '../core/layout.ts'
import { APP_URL } from '../core/origin.ts'
import { closeSocketsWithDocument } from '../core/pagesockets.ts'
import { Tickets } from '../core/tickets.ts'
import { TITLE, windowTitle } from '../core/title.ts'
import { orphanPartitions, wipeWithRetry } from '../core/wipe.ts'
import { LEGACY_PARTITION, PARTITIONS, serverPartition, type Context } from './context.ts'
import { buildMenu } from './menu.ts'
import { setLauncherBadge, toastsClosed } from './notify.ts'
import { prepare, prepareServer } from './sessions.ts'
import { appHandler, assetHandler, HOST_PAGE_CSP } from './protocol.ts'
import { loadAssets } from './assets.ts'
import { pageShown, railShown } from '../core/visibility.ts'
import { appPreload, harden, pagePreferences, railPreload } from './windows.ts'

export const RAIL_WIDTH = 68

// ------------------------------------------------------------------- layout

function frontKey(ctx: Context): string | null {
  const registry = ctx.trusted.registry
  const active = registry.active()
  return active !== null && !registry.adding() ? active : null
}

/** Puts the rail on the left, the page in front next to it and hides the others (all of them while the window is minimised). Hidden pages keep running, throttled: the engine stops painting them and slows their timers. */
export function layout(ctx: Context): void {
  const { window, rail, selector, pages } = ctx.shell
  if (!window || window.isDestroyed()) return
  const { width, height } = window.getContentBounds()
  const minimized = window.isMinimized()
  rail?.setBounds({ x: 0, y: 0, width: RAIL_WIDTH, height })
  rail?.setVisible(railShown(minimized))
  const content = { x: RAIL_WIDTH, y: 0, width: Math.max(1, width - RAIL_WIDTH), height }
  const front = frontKey(ctx)
  if (selector) { selector.setBounds(content); selector.setVisible(pageShown(null, front, minimized)) }
  for (const [id, view] of pages) { view.setBounds(content); view.setVisible(pageShown(id, front, minimized)) }
}

export function focusFront(ctx: Context): void {
  const front = frontKey(ctx)
  const view = front === null ? ctx.shell.selector : ctx.shell.pages.get(front)
  if (view && !view.webContents.isDestroyed()) view.webContents.focus()
}

export function railSnapshot(ctx: Context) {
  return ctx.trusted.registry.rail(true)
}

/** What the rail was last told, so that a report that changes nothing (the page reports on every change) pushes nothing. */
const pushed = new WeakMap<Context, string>()

/** Shows the rail what it should draw. */
export function pushRail(ctx: Context): void {
  const snapshot = railSnapshot(ctx)
  const state = JSON.stringify(snapshot)
  if (pushed.get(ctx) === state) return
  pushed.set(ctx, state)
  const window = ctx.shell.window
  if (window && !window.isDestroyed()) window.setTitle(windowTitle(snapshot))
  const rail = ctx.shell.rail
  if (rail && !rail.webContents.isDestroyed()) {
    void rail.webContents.executeJavaScript(`window.mnemaRail&&window.mnemaRail.set(${state})`).catch(() => undefined)
  }
}

/** The counters changed: the launcher badge is their sum, the rail shows each. The menu does not show counters. */
export function countsChanged(ctx: Context): void {
  setLauncherBadge(ctx.trusted.registry.badge())
  pushRail(ctx)
}

/** The list or the order changed: save it, lay the pages out again and rename the menu entries to match. */
function changed(ctx: Context): void {
  ctx.trusted.saveInstances()
  layout(ctx)
  pushRail(ctx)
  buildMenu(ctx)
}

/** Shows the window of the shell (restoring it when minimized) and gives it the focus. */
export function raiseWindow(ctx: Context): void {
  const window = ctx.shell.window
  if (!window || window.isDestroyed()) return
  if (window.isMinimized()) window.restore()
  window.show()
  window.focus()
}

/** Brings a server to the front (a toast was used, the person chose it) and the window with it. */
export function bringForward(ctx: Context, id: string): void {
  if (!ctx.trusted.registry.activate(id)) return
  changed(ctx)
  raiseWindow(ctx)
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

/** Folders and local names: a change of how the servers sit on the rail. The id a new folder gets is the host's. A refused change is an error. */
export function editLayout(ctx: Context, edit: Edit): void {
  const registry = ctx.trusted.registry
  const fresh = (): string => `f-${randomBytes(4).toString('hex')}`
  let folder = fresh()
  while (registry.hasFolder(folder)) folder = fresh()
  if (!registry.edit(edit, folder)) throw new Error('The change was refused.')
  changed(ctx)
}

/** Asks a running server for its community icon, from the server itself, without holding anything up. */
function refreshIcon(ctx: Context, id: string): void {
  const { registry, icons } = ctx.trusted
  const slot = registry.slot(id)
  const session = registry.session(id)
  if (!slot || !session) return
  ctx.trusted.refreshIcon(id, slot.origin, session.iconWanted ?? true).then((moved) => {
    // A server removed while its icon was on the way takes the icon with it.
    if (!registry.contains(id)) icons.clear(id)
    else if (moved) pushRail(ctx)
  }, () => undefined)
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
  window.on('minimize', () => layout(ctx))
  window.on('restore', () => layout(ctx))
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

/** The page a server opens on. A recognised invite is the join screen; otherwise the app. */
function pageUrl(code: string | null): string {
  return code ? `${APP_URL}invite/${code}` : APP_URL
}

function atTheDoor(raw: string): boolean {
  try {
    const path = new URL(raw).pathname
    return path === '/' || path === '/login' || path === '/index.html' || path.startsWith('/invite/')
  } catch {
    return true
  }
}

/**
 * Opens the join screen of a server that is already in the list, when its
 * page has not gone into the app yet.
 */
function offerInvite(ctx: Context, id: string, code: string | null): void {
  if (!code) return
  const page = ctx.shell.pages.get(id)
  if (!page || page.webContents.isDestroyed() || !atTheDoor(page.webContents.getURL())) return
  void page.webContents.loadURL(pageUrl(code))
}

/** Creates the page of a running server, in a session of its own. */
function openPage(ctx: Context, id: string, code: string | null = null): void {
  if (ctx.shell.pages.has(id) || !ctx.shell.window) return
  const partition = prepareServer(ctx, id)
  const view = hostView(ctx, partition, appPreload())
  ctx.servers.set(view.webContents.id, id)
  closeSocketsWithDocument(view.webContents, () => ctx.trusted.registry.session(id)?.sockets)
  harden(ctx, view.webContents, 'instance', () => ctx.shell.window ?? undefined)
  view.webContents.once('destroyed', () => { if (ctx.shell.pages.get(id) === view) ctx.shell.pages.delete(id) })
  ctx.shell.pages.set(id, view)
  view.setVisible(false)
  ctx.shell.window.contentView.addChildView(view)
  void view.webContents.loadURL(pageUrl(code))
}

// ------------------------------------------------------------------- servers

const starts = new Tickets()

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
  // A start that is asked for again while it runs (the trust round, a reconnect) supersedes the first: only the newest one's result is used.
  const ticket = starts.begin(id)
  slot.phase = { kind: 'starting' }
  pushRail(ctx)
  try {
    const session = await ctx.trusted.connect(slot.origin)
    const current = registry.slot(id)
    if (!current || !starts.current(id, ticket)) { session.sockets.closeAll(); return }
    registry.restart(id)
    current.phase = { kind: 'ready', session }
    refreshIcon(ctx, id)
  } catch (error) {
    if (!starts.current(id, ticket)) return
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
  const code = inviteCode(address)
  const id = idFor(origin)
  if (registry.contains(id)) {
    offerInvite(ctx, id, code)
    bringForward(ctx, id)
    return
  }
  if (registry.length >= MAX_INSTANCES) throw new AddError('full')
  const session = await ctx.trusted.connect(origin)
  try {
    registry.add(origin)
  } catch (error) {
    session.sockets.closeAll()
    if (error instanceof AddError && error.kind === 'known') {
      offerInvite(ctx, id, code)
      bringForward(ctx, id)
      return
    }
    throw error
  }
  const slot = registry.slot(id)
  if (slot) { registry.restart(id); slot.phase = { kind: 'ready', session } }
  refreshIcon(ctx, id)
  registry.activate(id)
  try {
    openPage(ctx, id, code)
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
 * Removes a server's partition directory after its page stored what it stored
 * in the browser engine, in the background, and gives up if the server comes
 * back in the meantime.
 */
function wipeDirectory(ctx: Context, id: string, dir: string): void {
  wipeWithRetry(dir, { stop: () => ctx.trusted.registry.contains(id) })
}

/**
 * Takes a server out of the list and wipes everything this device holds of it:
 * its stored session (secret store entry and fallback file), its browser
 * storage, its sockets, its toasts and its attachment window (an in-memory session, so there is no directory of it).
 */
export async function removeServer(ctx: Context, id: string): Promise<void> {
  const { trusted } = ctx
  const partition = serverPartition(id)
  const slot = trusted.registry.remove(id)
  if (!slot) return
  if (slot.phase.kind === 'ready') {
    slot.phase.session.sockets.closeAll()
    slot.phase.session.instance.jar.forget()
  } else {
    trusted.forgetStoredSession(slot.origin)
  }
  trusted.icons.clear(id)
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
  changed(ctx)
  countsChanged(ctx)
  focusFront(ctx)
}

/**
 * Storage of servers that are no longer in the list (a removal cut short by the
 * end of the program) goes at the next start. Only when the list was read in
 * full: a list that could not be (unreadable, a newer version, an address that
 * no longer holds up) says nothing about which servers are gone, and the end-to-end
 * state of the others lives in exactly these directories.
 */
function sweepOrphans(ctx: Context): void {
  if (!ctx.trusted.registry.readInFull) return
  const root = join(ctx.trusted.dataDirectory() ?? app.getPath('userData'), 'Partitions')
  let names: string[]
  try { names = readdirSync(root) } catch { return }
  for (const name of orphanPartitions(names, new Set(ctx.trusted.registry.ids()))) wipeWithRetry(join(root, name))
}

/** Removes the storage of the single-server client: its data, then its directory. */
export async function wipeLegacyStorage(ctx: Context): Promise<void> {
  const ses = session.fromPartition(LEGACY_PARTITION)
  await ses.clearStorageData().catch(() => undefined)
  await ses.clearCache().catch(() => undefined)
  wipeWithRetry(partitionDir(ctx, LEGACY_PARTITION))
}

/** Whether the single-server client left its storage behind (it is gone once wiped, so this is true once). */
export function legacyStorageWaits(ctx: Context): boolean {
  const dir = partitionDir(ctx, LEGACY_PARTITION)
  return existsSync(dir)
}
