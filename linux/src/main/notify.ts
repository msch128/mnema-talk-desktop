/**
 * OS notifications and the launcher badge for the instance window. Electron's
 * own Notification does the showing; a click brings the window forward and
 * tells the page which message the toast was about.
 */
import { Notification, app, type WebContents } from 'electron'
import { MAX_BODY, MAX_TITLE, ToastGate, badgeCount, cleanText, validId } from '../core/notify.ts'
import type { Context } from './context.ts'

const gate = new ToastGate()
/** Keep the shown toasts referenced until they are used or closed; a collected one never fires its click. */
const shown = new Set<Notification>()
/** The page callback that receives clicks, per window, with the next message index its Channel expects. */
const listeners = new Map<number, { channel: number, index: number }>()

function send(contents: WebContents, listener: { channel: number, index: number }, message: object): void {
  if (!contents.isDestroyed()) contents.send('mnema:callback', listener.channel, { index: listener.index++, message })
}

export function notifyOpen(contents: WebContents, channel: number): null {
  if (!listeners.has(contents.id)) contents.once('destroyed', () => listeners.delete(contents.id))
  listeners.set(contents.id, { channel, index: 0 })
  return null
}

export function notify(ctx: Context, contents: WebContents, title: string, body: string, channelId: string, messageId: string): null {
  if (!validId(channelId) || !validId(messageId)) throw new Error('Invalid notification target.')
  if (!Notification.isSupported() || !gate.allow(Date.now())) return null
  const toast = new Notification({ title: cleanText(title, MAX_TITLE), body: cleanText(body, MAX_BODY), silent: false })
  shown.add(toast)
  toast.on('click', () => {
    shown.delete(toast)
    const win = ctx.windows.instance
    if (win && !win.isDestroyed()) {
      if (win.isMinimized()) win.restore()
      win.show()
      win.focus()
    }
    const listener = listeners.get(contents.id)
    if (listener) send(contents, listener, { channel_id: channelId, message_id: messageId })
  })
  toast.on('close', () => shown.delete(toast))
  toast.show()
  return null
}

export function setBadge(count: number): null {
  app.setBadgeCount(badgeCount(count))
  return null
}
