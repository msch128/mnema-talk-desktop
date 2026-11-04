/**
 * The files of the verified bundle and the policy they are served under. The
 * page sees them as the files of its one origin.
 */
import type { Bundle } from './bundle.ts'
import { type Resp, plain } from './resp.ts'
import { type Version, compareVersions } from './version.ts'

const APP_CSP_REST = "img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; "
  + "connect-src 'self'; worker-src 'self' blob:; "
  + "frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'"

/**
 * Same policy as the server's SPA (`httpx.SPAContentSecurityPolicy`) since
 * mnema-talk 1.1.0: no inline styles, neither elements nor attributes. Only
 * bundles that were built for it get it (see `appCsp`).
 */
export const APP_CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; style-src-attr 'none'; " + APP_CSP_REST

/**
 * The policy of bundles before 1.1.0: their emoji picker writes a `<style>`
 * element and `style` attributes, so they need `'unsafe-inline'`. The shell
 * can still load such a bundle (a 1.0.x instance), and the strict policy
 * would show its picker unstyled.
 */
export const LEGACY_APP_CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; " + APP_CSP_REST

/** First bundle version that runs under `APP_CSP`. */
const STRICT_STYLE_FROM: Version = [1, 1, 0]

/** The policy for a verified bundle, by the version in its signed manifest. */
export function appCsp(version: Version): string {
  return compareVersions(version, STRICT_STYLE_FROM) >= 0 ? APP_CSP : LEGACY_APP_CSP
}

const MIME: Record<string, string> = {
  html: 'text/html; charset=utf-8',
  js: 'text/javascript; charset=utf-8',
  mjs: 'text/javascript; charset=utf-8',
  css: 'text/css; charset=utf-8',
  json: 'application/json',
  map: 'application/json',
  wasm: 'application/wasm',
  svg: 'image/svg+xml',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
  gif: 'image/gif',
  ico: 'image/x-icon',
  woff2: 'font/woff2',
  woff: 'font/woff',
  txt: 'text/plain; charset=utf-8',
  webmanifest: 'application/manifest+json',
  mp3: 'audio/mpeg',
  ogg: 'audio/ogg',
  oga: 'audio/ogg',
  wav: 'audio/wav',
}

export function mimeFor(name: string): string {
  const dot = name.lastIndexOf('.')
  if (dot < 0) return 'application/octet-stream'
  return MIME[name.slice(dot + 1).toLowerCase()] ?? 'application/octet-stream'
}

/**
 * A file of the verified bundle, or the app shell for client-side routes, as
 * `web/web.go` does for browsers. `path` is the URL path without the query.
 */
export function serveBundle(bundle: Bundle, method: string, path: string, csp = appCsp(bundle.version)): Resp {
  if (method !== 'GET' && method !== 'HEAD') {
    const resp = plain(405, 'method not allowed')
    resp.headers.push(['allow', 'GET, HEAD'])
    return resp
  }
  let name = path.replace(/^\/+/, '')
  let body = name === '' ? undefined : bundle.file(name)
  if (body === undefined) {
    if (name.startsWith('assets/')) return plain(404, 'not found')
    body = bundle.file('index.html')
    if (body === undefined) return plain(503, 'the bundle has no app shell')
    name = 'index.html'
  }
  const cache = name.startsWith('assets/') ? 'public, max-age=31536000, immutable' : 'no-cache'
  return {
    status: 200,
    headers: [
      ['content-type', mimeFor(name)],
      ['content-security-policy', csp],
      ['x-content-type-options', 'nosniff'],
      ['referrer-policy', 'no-referrer'],
      ['permissions-policy', 'camera=(self), geolocation=(), payment=(), usb=(), microphone=(self), display-capture=(self)'],
      ['cache-control', cache],
    ],
    body: method === 'HEAD' ? new Uint8Array() : body,
  }
}
