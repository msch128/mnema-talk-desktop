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
import { existsSync } from 'node:fs'
import { BaseWindow, WebContentsView, app } from 'electron'
import { inviteCode, parseAddress } from '../core/client.ts'
import { writeFileAtomic } from '../core/atomic.ts'
import { AddError, MAX_IMPORT_BYTES, MAX_INSTANCES, idFor, storedFrom, storedText } from '../core/instances.ts'
import { type RailEdit, exportTarget } from '../core/railui.ts'
import { APP_URL } from '../core/origin.ts'
import { closeSocketsWithDocument } from '../core/pagesockets.ts'
import { Tickets } from '../core/tickets.ts'
import { TITLE } from '../core/title.ts'
import { PARTITIONS, type Context } from './context.ts'
import { prepare, prepareServer } from './sessions.ts'
import { appHandler, assetHandler, HOST_PAGE_CSP } from './protocol.ts'
import { loadAssets } from './assets.ts'
import { bringForward, changed, countsChanged, focusFront, layout, pushRail } from './shelllayout.ts'
import { removeServer, sweepOrphans } from './shellwipe.ts'
import { appPreload, harden, pagePreferences, railPreload } from './windows.ts'

/**
 * A change the rail asked for: folders and local names (the id a new folder gets
 * is the host's), the mute of a server, marking it read, and taking the list
 * out and in. A refused change is an error. The answer is nothing, the path of
 * an exported list, or the number of servers an import added.
 */
export async function railEdit(ctx: Context, edit: RailEdit): Promise<null | string | number> {
  const registry = ctx.trusted.registry
  switch (edit.op) {
    case 'mute':
      if (!registry.mute(edit.id, edit.until)) throw new Error('The change was refused.')
      changed(ctx)
      countsChanged(ctx)
      return null
    case 'mark_read': markRead(ctx, edit.id); return null
    case 'export': return exportList(ctx)
    case 'import': return importList(ctx, edit.text)
    default: {
      const fresh = (): string => `f-${randomBytes(4).toString('hex')}`
      let folder = fresh()
      while (registry.hasFolder(folder)) folder = fresh()
      if (!registry.edit(edit, folder)) throw new Error('The change was refused.')
      changed(ctx)
      return null
    }
  }
}

/** Tells the page of a running server to mark everything read; it answers with its usual unread report. */
function markRead(ctx: Context, id: string): void {
  const page = ctx.shell.pages.get(id)
  if (!ctx.trusted.registry.session(id) || !page || page.webContents.isDestroyed()) throw new Error('The change was refused.')
  void page.webContents.executeJavaScript("window.dispatchEvent(new CustomEvent('mnema-desktop:mark-all-read'))").catch(() => undefined)
}

/** Writes the list (addresses, names, folders, order, mutes: nothing of a session) into the downloads folder and says where. */
function exportList(ctx: Context): string {
  const dirs = [app.getPath('downloads'), ctx.trusted.dataDirectory()]
  let failure: unknown = new Error('The list could not be saved.')
  for (const dir of dirs) {
    if (!dir) continue
    try {
      const path = exportTarget(dir, existsSync)
      writeFileAtomic(path, Buffer.from(storedText(ctx.trusted.registry.stored())), { dirMode: 0o700 })
      return path
    } catch (error) { failure = error }
  }
  throw failure
}

/** Takes in an exported list: new servers join and start like those restored at start, names, mutes and the layout are applied. */
function importList(ctx: Context, text: string): number {
  if (Buffer.byteLength(text, 'utf8') > MAX_IMPORT_BYTES) throw new Error('The list is too large.')
  let raw: unknown
  try { raw = JSON.parse(text) } catch { throw new Error('This is not a list of servers.') }
  const stored = storedFrom(raw)
  if (!stored) throw new Error('This is not a list of servers.')
  const added = ctx.trusted.registry.merge(stored)
  changed(ctx)
  countsChanged(ctx)
  for (const slot of added) void startServer(ctx, slot.id)
  return added.length
}

/** The quick switcher (Ctrl+K) is the rail page's: it lists the servers it knows and activates the one picked. */
export function quickSwitch(ctx: Context): void {
  const rail = ctx.shell.rail
  if (rail && !rail.webContents.isDestroyed()) void rail.webContents.executeJavaScript('window.mnemaRail&&window.mnemaRail.quickSwitch()').catch(() => undefined)
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

