/**
 * What a page may be granted by the engine, free of Electron so the rules can
 * be tested. Permissions go only to the instance window, only for the app's own
 * pages, and only for what the app uses.
 */
import { localOrigin, parseUrl } from './origin.ts'

/** Microphone, camera (`media`), screen share, notifications for messages, copying, the output device and fullscreen video. */
const GRANTED = new Set(['media', 'display-capture', 'notifications', 'clipboard-sanitized-write', 'speaker-selection', 'fullscreen'])

/**
 * Whether the engine may say yes. `role` is the kind of page (`instance` is the
 * only one that is granted anything). Microphone and camera go to the page
 * itself, never to a frame inside it: on this host the engine shows no prompt
 * of its own once the answer is yes, so the app's own page is the only
 * requester that may reach a device.
 */
export function permitted(permission: string, requestingUrl: string, role: string | undefined, mediaTypes: readonly string[] = [], isMainFrame = true): boolean {
  if (role !== 'instance' || !GRANTED.has(permission)) return false
  const url = parseUrl(requestingUrl)
  if (!url || localOrigin(url) === null) return false
  if (permission !== 'media') return true
  // `media` also names the capture of audio and video; nothing else hides behind it.
  return isMainFrame && mediaTypes.every((t) => t === 'audio' || t === 'video')
}
