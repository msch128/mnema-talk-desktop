/**
 * Pages made when needed and closed when idle (`core/sleep.ts` has the rules).
 * A server gets its page when it is first opened in this run: at start only
 * the server in front loads, an import adds rail entries and nothing more. A
 * page hidden for a while is closed again while more than a few run, unless it
 * says it is busy (`desktop_report_busy`). Until a server has a page, or after
 * its page was closed, the host keeps its counts (`core/feed.ts`).
 */
import { Notification } from 'electron'
import { Feed, mentionRise, mentionText } from '../core/feed.ts'
import { type PageState, closable, pagesToUnload } from '../core/sleep.ts'
import type { Context } from './context.ts'
import { countsChanged, layout } from './shelllayout.ts'

const LOOK_EVERY_MS = 30_000

interface Asleep { feeds: Map<string, Feed>, hidden: Map<string, number> }
const state = new WeakMap<Context, Asleep>()
function of(ctx: Context): Asleep {
  let s = state.get(ctx)
  if (!s) { s = { feeds: new Map(), hidden: new Map() }; state.set(ctx, s) }
  return s
}

const now = () => Math.floor(Date.now() / 1000)

/** The server in front, if a server (not the page for adding one) is. */
function front(ctx: Context): string | null {
  const registry = ctx.trusted.registry
  return registry.adding() ? null : registry.active()
}

/** Whether `id`'s page should exist now: it is the server in front. */
export function wantsPage(ctx: Context, id: string): boolean {
  return front(ctx) === id
}

/** A quiet notification for a server without a page: no sound; the desktop's notification service decides whether it shows (do not disturb, the app's setting). */
function quietToast(title: string, body: string): void {
  if (Notification.isSupported()) new Notification({ title, body, silent: true }).show()
}

/** Starts keeping the counts of a ready server that has no page. */
export function withoutPage(ctx: Context, id: string): void {
  const s = of(ctx)
  const session = ctx.trusted.registry.session(id)
  if (!session || s.feeds.has(id)) return
  const feed = new Feed(session.instance, (read) => {
    const registry = ctx.trusted.registry
    const slot = registry.slot(id)
    if (!slot || !registry.session(id) || read.kind === 'unreachable') return
    // A session that is gone shows nothing until its page signs in again.
    const { unread, mentions } = read.kind === 'counts' ? read : { unread: 0, mentions: 0 }
    if (read.kind === 'signed_out') s.feeds.delete(id)
    const before = slot.mentions
    if (!registry.report(id, unread, mentions)) return
    countsChanged(ctx)
    const rise = mentionRise(before, slot.mentions, registry.isMuted(id))
    if (rise !== null) quietToast(registry.displayName(slot), mentionText(rise, registry.displayName(slot)))
  })
  s.feeds.set(id, feed)
  feed.start()
}

/** Stops keeping the counts of `id` (its page took over, or it went away). */
export function stopFeed(ctx: Context, id: string): void {
  const s = of(ctx)
  s.feeds.get(id)?.stop()
  s.feeds.delete(id)
}

/** After a placement: notes which pages are hidden since when, and makes the page of the server in front if it has none. */
export function afterLayout(ctx: Context, open: (id: string) => void): void {
  const s = of(ctx)
  const inFront = front(ctx)
  for (const id of [...s.hidden.keys()]) if (!ctx.shell.pages.has(id)) s.hidden.delete(id)
  for (const id of ctx.shell.pages.keys()) {
    if (id === inFront) s.hidden.delete(id)
    else if (!s.hidden.has(id)) s.hidden.set(id, now())
  }
  if (inFront !== null && !ctx.shell.pages.has(inFront) && ctx.trusted.registry.session(inFront)) {
    stopFeed(ctx, inFront)
    open(inFront)
  }
}

/** Closes the hidden pages the rules say may go, now. */
export function look(ctx: Context): void {
  const s = of(ctx)
  const registry = ctx.trusted.registry
  const inFront = front(ctx)
  const servers: PageState[] = registry.ids().map((id) => ({ id, active: id === inFront, page: ctx.shell.pages.has(id), hiddenSince: s.hidden.get(id) ?? null, busy: registry.slot(id)?.busy ?? null }))
  for (const id of pagesToUnload(registry.sleep, now(), servers)) unload(ctx, id)
}

/** Closes the page of `id`: the host's feed starts first, so the member never reads as offline in between, then the page and its sockets go. */
function unload(ctx: Context, id: string): void {
  const session = ctx.trusted.registry.session(id)
  const page = ctx.shell.pages.get(id)
  const slot = ctx.trusted.registry.slot(id)
  // Checked again right before closing: the page may have become busy, or its server the one in front.
  if (!session || !page || !slot || !closable(front(ctx) === id, slot.busy)) return
  // The next page is a new run: it says again whether it is busy.
  slot.busy = null
  withoutPage(ctx, id)
  ctx.shell.pages.delete(id)
  ctx.shell.window?.contentView.removeChildView(page)
  page.webContents.close()
  session.sockets.closeAll()
  of(ctx).hidden.delete(id)
  layout(ctx)
}

/** Looks at the hidden pages every half minute for as long as the window lives. */
export function watch(ctx: Context): () => void {
  const timer = setInterval(() => look(ctx), LOOK_EVERY_MS)
  return () => clearInterval(timer)
}
