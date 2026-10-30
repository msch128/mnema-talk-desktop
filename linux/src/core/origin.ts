import { appPath } from './client.ts'

/** The app is served at `mnema://localhost/` (a privileged, secure scheme of the main process). */
export const APP_SCHEME = 'mnema'
export const APP_HOST = 'localhost'
export const APP_URL = `${APP_SCHEME}://${APP_HOST}/`

/** The host's own update page lives under this prefix of the same origin. */
export const UI_PREFIX = '/__update/'
/** The screen picker of the host, likewise. */
export const PICKER_PREFIX = '/__picker/'

/**
 * `scheme://host` of a URL on the app origin, else null. (The origin of a
 * custom scheme can be opaque and never compare equal, so origins are
 * compared as text.)
 */
export function localOrigin(url: URL): string | null {
  return url.protocol === `${APP_SCHEME}:` && url.hostname === APP_HOST && url.port === '' && url.username === '' && url.password === ''
    ? `${APP_SCHEME}://${APP_HOST}`
    : null
}

/** An app page: the app origin and one of the app's own routes. */
export function bundled(url: URL): boolean {
  return localOrigin(url) !== null && appPath(url.pathname)
}

/** A path the proxy serves for the selected instance, on the app origin. */
export function appApi(url: URL): boolean {
  return localOrigin(url) !== null && url.pathname.startsWith('/api/')
}

/** Only the update page itself may be shown in the update window. */
export function updateUi(url: URL): boolean {
  return localOrigin(url) !== null && url.pathname.startsWith(UI_PREFIX)
}

/** The small dialog of the rail (rename, confirm): its one page, nothing else. */
export function dialogUi(url: URL): boolean {
  return localOrigin(url) !== null && url.pathname === '/dialog.html'
}

export function pickerUi(url: URL): boolean {
  return localOrigin(url) !== null && url.pathname.startsWith(PICKER_PREFIX)
}

/** Parses without throwing; `null` for anything that is not a URL. */
export function parseUrl(text: string): URL | null {
  try {
    return new URL(text)
  } catch {
    return null
  }
}
