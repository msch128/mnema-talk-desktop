/**
 * OS notifications and the launcher badge for the pages of the servers.
 * Electron's own Notification does the showing; a toast names its server once
 * there are several, and using it brings that server forward and tells its
 * page (and no other) which message the toast was about.
 */
import { Notification, app, type WebContents } from 'electron'
import { MAX_BODY, ToastGates, badgeCount, cleanText, taggedTitle, validId } from '../core/notify.ts'
import { nameOf } from '../core/instances.ts'
import type { Context } from './context.ts'
import { bringForward } from './shell.ts'

const gates = new ToastGates()
/** Keep the shown toasts referenced until they are used or closed; a collected one never fires its click. The value is the generation of the page run it belongs to. */
const shown = new Map<Notification, string>()

/** What the web app calls permission: the desktop's notification service either exists or does not. */
export function notifyPermission(): 'granted' | 'unsupported' {
  return Notification.isSupported() ? 'granted' : 'unsupported'
}

export function notifyOpen(ctx: Context, server: string, contents: WebContents, channel: number): null {
  const slot = ctx.trusted.registry.slot(server)
  if (!slot) throw new Error('No instance is selected.')
  if (slot.clicks === null) {
    const generation = slot.generation
    contents.once('destroyed', () => {
      // Toasts still showing belong to a page that is gone: take them down, they must not be usable anymore.
      for (const [toast, owner] of shown) if (owner === generation) { shown.delete(toast); toast.close() }
    })
  }
  slot.clicks = { channel, index: 0 }
  return null
}

export function notify(ctx: Context, server: string, title: string, body: string, channelId: string, messageId: string): null {
  if (!validId(channelId) || !validId(messageId)) throw new Error('Invalid notification target.')
  const registry = ctx.trusted.registry
  const slot = registry.slot(server)
  if (!slot) throw new Error('No instance is selected.')
  if (!Notification.isSupported() || !gates.allow(server, Date.now())) return null
  const generation = slot.generation
  const toast = new Notification({ title: taggedTitle(title, nameOf(slot), registry.length > 1), body: cleanText(body, MAX_BODY), silent: false })
  shown.set(toast, generation)
  toast.on('click', () => {
    shown.delete(toast)
    // A toast of a server that is no longer on the list, or of an earlier run of its page, is dropped: its ids mean nothing to any other page.
    const current = registry.byGeneration(generation)
    if (!current) return
    bringForward(ctx, current.id)
    const page = ctx.shell.pages.get(current.id)
    const listener = current.clicks
    if (page && listener && !page.webContents.isDestroyed()) {
      page.webContents.send('mnema:callback', listener.channel, { index: listener.index++, message: { channel_id: channelId, message_id: messageId } })
    }
  })
  toast.on('close', () => shown.delete(toast))
  toast.show()
  return null
}

/** A server left the list: its toasts go, and so does its gate. */
export function toastsClosed(server: string, generation: string): void {
  gates.forget(server)
  for (const [toast, owner] of shown) if (owner === generation) { shown.delete(toast); toast.close() }
}

/** The launcher badge: the sum of the mentions over all servers. */
export function setLauncherBadge(count: number): void {
  app.setBadgeCount(badgeCount(count))
}

