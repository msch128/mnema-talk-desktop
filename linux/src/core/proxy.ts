/**
 * What the app origin serves: the verified bundle's files, and a proxy to the
 * selected instance for `/api/`. The page sees one origin, as in a browser; the
 * session cookie stays here and the instance's answers are data, never code
 * (mnema-talk `doc/desktop-trusted-client.md`). A port of
 * `experimental/app/src/web_proxy.rs`; the shared vectors must pass in both.
 */
import type { Agent } from 'node:http'
import type { Bundle } from './bundle.ts'
import type { Jar } from './cookies.ts'
import { defaultAgent, headerText, readBody, send } from './http.ts'
import { type Version, compareVersions } from './version.ts'

/**
 * Largest body the proxy buffers before the instance has announced its upload
 * limit (`max_upload_mb` of `GET /api/permissions`). Every byte is in memory;
 * the caps keep one request from taking the machine's RAM. Audio and video
 * arrive in `Range` pieces and are not affected.
 */
export const MAX_BODY = 128 << 20
/** What the server's JSON routes accept (1 MiB), with headroom. */
const MAX_JSON_BODY = 2 << 20
/** Multipart framing and form fields around an uploaded file. */
const MULTIPART_OVERHEAD = 1 << 20
/** Avatars are read in memory by the server and capped at 5 MiB. */
const MAX_AVATAR_BODY = (5 << 20) + MULTIPART_OVERHEAD
/** The server's own ceiling (`MAX_UPLOAD_SIZE_MB` is at most 2048). */
const MAX_ANNOUNCED_MB = 2048
/** Bodies up to this size are handled without asking the memory budget. */
export const SMALL_BODY = 32 << 20
/** Memory the proxy lends to bodies above `SMALL_BODY`, all transfers together. */
export const LARGE_BODY_BUDGET = 1 << 30
const BUDGET_UNIT = 64 << 10
const BUDGET_WAIT_MS = 30_000

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

/**
 * Applied to everything that comes from the instance. `sandbox` (without
 * `allow-same-origin` or `allow-scripts`) gives a document opened from such a
 * response an opaque origin without script, whatever the server claimed.
 */
const DATA_CSP = "sandbox; default-src 'none'"

export interface Resp {
  status: number
  headers: [string, string][]
  body: Uint8Array
}

export function plain(status: number, text: string): Resp {
  return {
    status,
    headers: [['content-type', 'text/plain; charset=utf-8'], ['x-content-type-options', 'nosniff'], ['cache-control', 'no-store']],
    body: Buffer.from(text),
  }
}

/** The server's error envelope, so the app's error handling applies. */
export function errorResp(status: number, code: string, message: string): Resp {
  return {
    status,
    headers: [['content-type', 'application/json'], ['x-content-type-options', 'nosniff'], ['cache-control', 'no-store']],
    body: Buffer.from(JSON.stringify({ error: { code, message } })),
  }
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

// ----------------------------------------------------------------- limits

class Budget {
  private free: number
  private readonly capacity: number
  private readonly waiting: { units: number, grant: () => void }[] = []

  constructor(capacity: number) {
    this.capacity = capacity
    this.free = capacity
  }

  /** Waits for `units`; resolves to a release function, or null after `timeoutMs`. */
  acquire(units: number, timeoutMs: number): Promise<(() => void) | null> {
    if (units > this.capacity) return Promise.resolve(null)
    return new Promise((resolve) => {
      const release = () => { this.free += units; this.drain() }
      const waiter = { units, grant: () => { clearTimeout(timer); this.free -= units; resolve(release) } }
      const timer = setTimeout(() => {
        const at = this.waiting.indexOf(waiter)
        if (at >= 0) this.waiting.splice(at, 1)
        resolve(null)
      }, timeoutMs)
      this.waiting.push(waiter)
      this.drain()
    })
  }

  private drain(): void {
    // First come, first served: a large request is not overtaken forever.
    while (this.waiting.length > 0 && (this.waiting[0] as { units: number }).units <= this.free) {
      (this.waiting.shift() as { grant: () => void }).grant()
    }
  }
}

/** What the proxy lets through, following what the instance announces. */
export class Limits {
  /** The effective upload limit of the signed-in member in MB; 0 until the instance has announced one. */
  private uploadMb = 0
  private readonly budget: Budget
  private readonly waitMs: number

  constructor(waitMs = BUDGET_WAIT_MS, capacity = LARGE_BODY_BUDGET) {
    this.waitMs = waitMs
    this.budget = new Budget(Math.floor(capacity / BUDGET_UNIT))
  }

  /** Takes the instance's announcement; nonsense is ignored. */
  announceUploadMb(mb: number): void {
    if (Number.isSafeInteger(mb) && mb >= 1 && mb <= MAX_ANNOUNCED_MB) this.uploadMb = mb
  }

  private uploadBytes(): number | null {
    return this.uploadMb === 0 ? null : this.uploadMb * 2 ** 20
  }

  /**
   * Largest request body for `path` (without the query): an upload follows the
   * announced limit plus multipart framing, avatars and JSON routes have their
   * own small caps. Before any announcement the fixed cap applies to uploads.
   */
  requestCap(path: string): number {
    switch (routeOf(path)) {
      case 'avatar': return MAX_AVATAR_BODY
      case 'json': return MAX_JSON_BODY
      case 'upload': {
        // Never above what the memory budget can lend: a larger body could
        // only wait for permits that do not exist.
        const n = this.uploadBytes()
        return Math.min(n === null ? MAX_BODY : n + MULTIPART_OVERHEAD, LARGE_BODY_BUDGET)
      }
    }
  }

  /**
   * Largest answer the proxy hands to the page: files up to the larger of the
   * old cap and the upload limit (a file stays downloadable after the Owner
   * lowered the limit), bounded by the memory budget.
   */
  responseCap(): number {
    const n = this.uploadBytes()
    const announced = n === null ? 0 : n + MULTIPART_OVERHEAD
    return Math.min(Math.max(MAX_BODY, announced), LARGE_BODY_BUDGET)
  }

  /** Lends memory for a body of `len` bytes. Small bodies take nothing; a large one waits for others to finish. `null` means no memory was available. */
  async lend(len: number): Promise<(() => void) | null> {
    if (len <= SMALL_BODY) return () => {}
    const units = Math.min(Math.ceil(len / BUDGET_UNIT), 0xffffffff)
    return this.budget.acquire(units, this.waitMs)
  }
}

// ------------------------------------------------------------------ proxy

/** The selected instance. */
export interface Instance {
  readonly origin: URL
  readonly jar: Jar
  readonly limits: Limits
  readonly agent?: Agent
  now(): number
}

const FORWARDED_REQUEST = new Set(['content-type', 'accept', 'accept-language', 'range', 'if-none-match', 'if-modified-since', 'x-request-id'])

const FORWARDED_RESPONSE = new Set(['content-disposition', 'etag', 'last-modified', 'cache-control', 'content-range', 'accept-ranges', 'content-language', 'retry-after', 'x-request-id'])

/**
 * Media types the page may receive as such. Anything that could be a document
 * or a script in the app origin (HTML, SVG, XML, JavaScript, ...) is handed
 * over as a download instead.
 */
export function inlineSafe(contentType: string): boolean {
  const essence = (contentType.split(';')[0] ?? '').trim().toLowerCase()
  return [
    'application/json', 'text/plain', 'application/octet-stream', 'image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif',
    'image/bmp', 'image/x-icon', 'image/vnd.microsoft.icon', 'image/apng',
  ].includes(essence)
    || essence.startsWith('audio/')
    || essence.startsWith('video/')
    || (essence.startsWith('application/') && essence.endsWith('+json'))
}

/**
 * `pathAndQuery` as the page sent it. Only `/api/` of the instance is
 * reachable, whatever dot segments or encodings the path contains.
 */
export function target(origin: URL, pathAndQuery: string): URL | null {
  if (!pathAndQuery.startsWith('/api/') || [...pathAndQuery].some((c) => isControl(c) || c === '\\')) return null
  let url: URL
  try {
    url = new URL(pathAndQuery, origin)
  } catch {
    return null
  }
  return url.origin === origin.origin && url.pathname.startsWith('/api/') ? url : null
}

function isControl(c: string): boolean {
  const code = c.codePointAt(0) as number
  return code < 0x20 || (code >= 0x7f && code <= 0x9f)
}

/** The answer to a request body above the cap `requestCap(path)` gave. */
export function payloadTooLarge(path: string, cap: number): Resp {
  if (routeOf(path) === 'json') return errorResp(413, 'PAYLOAD_TOO_LARGE', 'request too large')
  return errorResp(413, 'PAYLOAD_TOO_LARGE', `file exceeds the ${(cap - Math.min(MULTIPART_OVERHEAD, cap)) >> 20} MB limit`)
}

/** Largest piece of a file asked for in one `Range` request; the answer is read whole. */
export const RANGE_WINDOW = 8 * 2 ** 20

/**
 * `value` of a `Range` header with a single byte range cut to `RANGE_WINDOW`.
 * Anything else (several ranges, another unit, nonsense) goes through unchanged.
 */
export function clampRange(value: string): string {
  const spec = value.trim().startsWith('bytes=') ? value.trim().slice('bytes='.length).trim() : null
  const dash = spec === null ? -1 : spec.indexOf('-')
  if (spec === null || dash < 0) return value
  const first = spec.slice(0, dash).trim()
  const last = spec.slice(dash + 1).trim()
  const max = 2n ** 64n - 1n
  const number = (text: string): bigint | null => {
    if (!/^[0-9]+$/.test(text)) return null
    const n = BigInt(text)
    return n <= max ? n : null
  }
  const window = BigInt(RANGE_WINDOW)
  const cut = (n: bigint): bigint => (n > max ? max : n)
  const [a, b] = [number(first), number(last)]
  if (first === '' && b !== null) return `bytes=-${b < window ? b : window}`
  if (a !== null && last === '') return `bytes=${a}-${cut(a + window - 1n)}`
  if (a !== null && b !== null && b >= a) return `bytes=${a}-${b < cut(a + window - 1n) ? b : cut(a + window - 1n)}`
  return value
}

type Route = 'avatar' | 'upload' | 'json'

function routeOf(path: string): Route {
  if (path === '/api/users/me/avatar') return 'avatar'
  const rest = path.startsWith('/api/channels/') ? path.slice('/api/channels/'.length) : null
  const id = rest !== null && rest.endsWith('/upload') ? rest.slice(0, -'/upload'.length) : null
  return id !== null && id !== '' && !id.includes('/') ? 'upload' : 'json'
}

const METHODS = new Set(['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE'])

export async function forward(
  instance: Instance,
  method: string,
  pathAndQuery: string,
  headers: readonly (readonly [string, string])[],
  body: Uint8Array,
): Promise<Resp> {
  const url = target(instance.origin, pathAndQuery)
  if (!url) return errorResp(400, 'INVALID_INPUT', 'unsupported path')
  if (!METHODS.has(method)) return errorResp(405, 'INVALID_INPUT', 'method not allowed')
  const cap = instance.limits.requestCap(url.pathname)
  if (body.length > cap) return payloadTooLarge(url.pathname, cap)
  // Held until the answer is complete: the body stays in memory until the
  // instance has read it.
  const requestMemory = await instance.limits.lend(body.length)
  if (!requestMemory) return errorResp(503, 'UNAVAILABLE', 'too many large transfers at once, try again')
  try {
    return await exchange(instance, method, url, headers, body)
  } finally {
    requestMemory()
  }
}

async function exchange(instance: Instance, method: string, url: URL, headers: readonly (readonly [string, string])[], body: Uint8Array): Promise<Resp> {
  const announcesLimit = url.pathname === '/api/permissions' && method === 'GET'
  const out: Record<string, string> = {}
  for (const [name, value] of headers) {
    const lower = name.toLowerCase()
    if (lower === 'range') out.range = clampRange(value)
    else if (FORWARDED_REQUEST.has(lower)) out[lower] = value
  }
  // The page cannot choose these: the host speaks as the browser would on the
  // instance's own origin.
  out.origin = instance.origin.origin
  const cookie = instance.jar.header(instance.now())
  if (cookie) out.cookie = cookie
  let response
  try {
    response = await send({
      method,
      url,
      headers: out,
      body: method !== 'GET' && method !== 'HEAD' ? body : null,
      connectTimeoutMs: 10_000,
      totalTimeoutMs: 600_000,
      agent: instance.agent ?? (url.protocol === 'https:' ? defaultAgent() : undefined),
    })
  } catch {
    return errorResp(502, 'UNAVAILABLE', 'the server could not be reached')
  }
  const status = response.status
  instance.jar.store(response.setCookie, instance.origin.protocol === 'https:', instance.now())
  // 304 answers the conditional request the proxy passes on; it is no redirect.
  if (status >= 300 && status < 400 && status !== 304) {
    response.destroy()
    return errorResp(502, 'UNAVAILABLE', 'the server answered with a redirect')
  }
  const responseCap = instance.limits.responseCap()
  if (response.contentLength !== null && response.contentLength > responseCap) {
    response.destroy()
    return errorResp(502, 'UNAVAILABLE', "the server's answer is too large for this client")
  }
  // A large answer borrows its memory first; one of unknown length is held to
  // the small-body size.
  const [reserve, readCap] = response.contentLength !== null ? [response.contentLength, responseCap] : [0, SMALL_BODY]
  const answerMemory = await instance.limits.lend(reserve)
  if (!answerMemory) {
    response.destroy()
    return errorResp(503, 'UNAVAILABLE', 'too many large transfers at once, try again')
  }
  try {
    const bytes = await readBody(response, readCap)
    if (!bytes) return errorResp(502, 'UNAVAILABLE', "the server's answer was cut off or too large")
    if (announcesLimit && status === 200) {
      const mb = announcedUploadMb(bytes)
      if (mb !== null) instance.limits.announceUploadMb(mb)
    }
    const forwarded: [string, string][] = []
    for (const [name, value] of Object.entries(response.headers)) {
      const text = headerText(value)
      if (FORWARDED_RESPONSE.has(name) && text !== undefined) forwarded.push([name, text])
    }
    const declared = headerText(response.headers['content-type']) ?? 'application/octet-stream'
    if (inlineSafe(declared)) {
      forwarded.push(['content-type', declared])
    } else {
      forwarded.push(['content-type', 'application/octet-stream'])
      if (!forwarded.some(([name]) => name === 'content-disposition')) forwarded.push(['content-disposition', 'attachment'])
    }
    forwarded.push(['content-security-policy', DATA_CSP], ['x-content-type-options', 'nosniff'], ['cross-origin-resource-policy', 'same-origin'])
    return { status, headers: forwarded, body: method === 'HEAD' ? new Uint8Array() : bytes }
  } finally {
    answerMemory()
  }
}

function announcedUploadMb(body: Uint8Array): number | null {
  try {
    const value: unknown = JSON.parse(Buffer.from(body).toString('utf8'))
    if (typeof value === 'object' && value !== null && 'max_upload_mb' in value) {
      const mb = (value as { max_upload_mb: unknown }).max_upload_mb
      return typeof mb === 'number' && Number.isSafeInteger(mb) && mb >= 0 ? mb : null
    }
  } catch { /* not JSON */ }
  return null
}
