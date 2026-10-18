/**
 * What a session may do. Pages never reach the network themselves: every
 * request is answered by the host (`mnema:`), except `blob:` and `data:` URLs
 * the page makes itself. Permissions are granted only to the instance window,
 * only for the app's own pages, and only for what the app uses.
 */
import { type DesktopCapturerSource, type Session, type WebContents, desktopCapturer } from 'electron'
import { localOrigin, parseUrl } from '../core/origin.ts'
import type { Context, Role } from './context.ts'
import { openPicker } from './windows.ts'

/** Microphone, camera (`media`), screen share, notifications for messages, copying, the output device and fullscreen video. */
const GRANTED = new Set(['media', 'display-capture', 'notifications', 'clipboard-sanitized-write', 'speaker-selection', 'fullscreen'])

const NETWORK_FREE_SCHEMES = new Set(['mnema:', 'blob:', 'data:', 'devtools:'])

export function permitted(permission: string, requestingUrl: string, role: Role | undefined, mediaTypes: readonly string[] = []): boolean {
  if (role !== 'instance' || !GRANTED.has(permission)) return false
  const url = parseUrl(requestingUrl)
  if (!url || localOrigin(url) === null) return false
  // `media` also names the capture of audio and video; nothing else hides behind it.
  return permission !== 'media' || mediaTypes.every((t) => t === 'audio' || t === 'video')
}

export function secureSession(ctx: Context, ses: Session, role: Role): void {
  ses.setPermissionRequestHandler((contents: WebContents | null, permission, callback, details) => {
    const mediaTypes = 'mediaTypes' in details && Array.isArray(details.mediaTypes) ? (details.mediaTypes as string[]) : []
    callback(contents !== null && permitted(permission, details.requestingUrl, ctx.roles.get(contents.id), mediaTypes))
  })
  ses.setPermissionCheckHandler((contents, permission, requestingOrigin) => {
    return contents !== null && permitted(permission, requestingOrigin, ctx.roles.get(contents.id))
  })
  // No USB, HID, serial or Bluetooth device ever.
  ses.setDevicePermissionHandler(() => false)
  ses.setBluetoothPairingHandler((_details, callback) => callback({ confirmed: false }))
  ses.webRequest.onBeforeRequest((details, callback) => {
    const url = parseUrl(details.url)
    callback({ cancel: url === null || !NETWORK_FREE_SCHEMES.has(url.protocol) })
  })
  ses.setSpellCheckerEnabled(false)
  if (role === 'instance') {
    ses.setDisplayMediaRequestHandler((request, callback) => {
      void chooseScreen(ctx, request.frame?.url ?? '', request.userGesture).then((source) => callback(source ? { video: source } : null))
    })
  } else {
    ses.setDisplayMediaRequestHandler((_request, callback) => callback(null))
  }
}

/**
 * The screen or window to share. Under Wayland the portal asks the user and
 * `getSources` returns just that choice; elsewhere the host shows its own list.
 */
async function chooseScreen(ctx: Context, frameUrl: string, gesture: boolean): Promise<DesktopCapturerSource | null> {
  const url = parseUrl(frameUrl)
  const win = ctx.windows.instance
  if (!gesture || !url || localOrigin(url) === null || !win || win.isDestroyed()) return null
  const sources = await desktopCapturer.getSources({ types: ['screen', 'window'], thumbnailSize: { width: 320, height: 180 }, fetchWindowIcons: true }).catch(() => [])
  if (sources.length <= 1) return sources[0] ?? null
  return pick(ctx, win, sources)
}

interface PendingPick {
  sources: DesktopCapturerSource[]
  resolve(source: DesktopCapturerSource | null): void
}
const pending = new WeakMap<Context, PendingPick>()

function pick(ctx: Context, parent: Electron.BrowserWindow, sources: DesktopCapturerSource[]): Promise<DesktopCapturerSource | null> {
  pending.get(ctx)?.resolve(null)
  return new Promise((resolve) => {
    let done = false
    const finish = (source: DesktopCapturerSource | null) => {
      if (done) return
      done = true
      pending.delete(ctx)
      resolve(source)
      const win = ctx.windows.picker
      if (win && !win.isDestroyed()) win.close()
    }
    pending.set(ctx, { sources, resolve: finish })
    openPicker(ctx, parent).once('closed', () => finish(null))
  })
}

/** What the picker window shows: names and thumbnails, nothing it could use to capture on its own. */
export function pickerSources(ctx: Context): { id: string, name: string, thumbnail: string, screen: boolean }[] {
  return (pending.get(ctx)?.sources ?? []).map((s) => ({ id: s.id, name: s.name, thumbnail: s.thumbnail.toDataURL(), screen: s.id.startsWith('screen:') }))
}

export function pickerChoose(ctx: Context, id: string | null): void {
  const entry = pending.get(ctx)
  if (!entry) return
  entry.resolve(entry.sources.find((s) => s.id === id) ?? null)
}
