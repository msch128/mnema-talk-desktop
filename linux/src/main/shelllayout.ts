/**
 * Where the pieces of the shell window stand: the rail on the left, the page in
 * front next to it, and the commands that change what is in front. Every change
 * to the list or the order saves it, lays the pages out again and tells the rail.
 */
import { setLauncherBadge } from './notify.ts'
import { buildMenu } from './menu.ts'
import { pageShown, railShown } from '../core/visibility.ts'
import { windowTitle } from '../core/title.ts'
import type { Context } from './context.ts'
import { Coalesce, PUSH_AFTER_MS } from '../core/coalesce.ts'

export const RAIL_WIDTH = 68

/** What runs before each placement (the shell makes the page of the server in front there). */
const afterLayoutHooks: ((ctx: Context) => void)[] = []
export function onLayout(hook: (ctx: Context) => void): void {
  afterLayoutHooks.push(hook)
}

function frontKey(ctx: Context): string | null {
  const registry = ctx.trusted.registry
  const active = registry.active()
  return active !== null && !registry.adding() ? active : null
}

/** Puts the rail on the left, the page in front next to it and hides the others (all of them while the window is minimised). Hidden pages keep running, throttled: the engine stops painting them and slows their timers. */
export function layout(ctx: Context): void {
  for (const hook of afterLayoutHooks) hook(ctx)
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

const pushes = new WeakMap<Context, Coalesce>()

/** Shows the rail what it should draw: shortly, together with the other changes of that moment (`Coalesce`). */
export function pushRail(ctx: Context): void {
  let coalesce = pushes.get(ctx)
  if (!coalesce) { coalesce = new Coalesce(() => pushNow(ctx), PUSH_AFTER_MS); pushes.set(ctx, coalesce) }
  coalesce.request()
}

function pushNow(ctx: Context): void {
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
export function changed(ctx: Context): void {
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
  ctx.trusted.registry.startAdding()
  changed(ctx)
  focusFront(ctx)
}

export function cancelAdding(ctx: Context): void {
  ctx.trusted.registry.stopAdding()
  changed(ctx)
  focusFront(ctx)
}
