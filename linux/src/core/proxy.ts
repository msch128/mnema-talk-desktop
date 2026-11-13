/**
 * What the app origin serves: the verified bundle's files, and a proxy to the
 * selected instance for `/api/`. The page sees one origin, as in a browser; the
 * session cookie stays here and the instance's answers are data, never code
 * (mnema-talk `doc/desktop-trusted-client.md`). A port of
 * `experimental/app/src/web_proxy.rs`; the shared vectors must pass in both.
 */
import type { Agent } from 'node:http'
import type { Jar } from './cookies.ts'
import { defaultAgent, headerText, readBody, send, type HttpResponse } from './http.ts'
import { type Limits, MULTIPART_OVERHEAD, SMALL_BODY } from './proxylimits.ts'
import { DATA_CSP, type Resp, errorResp } from './resp.ts'

export { LARGE_BODY_BUDGET, Limits, MAX_BODY, SMALL_BODY } from './proxylimits.ts'
export { type Resp, plain } from './resp.ts'
export { APP_CSP, LEGACY_APP_CSP, appCsp, mimeFor, serveBundle } from './staticserve.ts'

/** The selected instance. */
export interface Instance {
  readonly origin: URL
  readonly jar: Jar
  readonly limits: Limits
  readonly agent?: Agent
  now(): number
}

const FORWARDED_REQUEST = new Set(['content-type', 'accept', 'accept-language', 'range', 'if-range', 'if-none-match', 'if-modified-since', 'x-request-id'])

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

/** The answer to a request body above the cap, in the server's own wording. */
export function payloadTooLarge(cap: number, file = true): Resp {
  if (!file) return errorResp(413, 'PAYLOAD_TOO_LARGE', 'request too large')
  // Not `>>`: that is a 32-bit shift and turns the 2 GiB maximum negative.
  return errorResp(413, 'PAYLOAD_TOO_LARGE', `file exceeds the ${Math.floor((cap - Math.min(MULTIPART_OVERHEAD, cap)) / 2 ** 20)} MB limit`)
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
  const { cap, file } = instance.limits.bodyLimit(url.pathname)
  if (body.length > cap) return payloadTooLarge(cap, file)
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

/** What of the page's request headers goes to the instance, and the headers only the host may set. */
export function requestHeaders(instance: Instance, headers: readonly (readonly [string, string])[]): Record<string, string> {
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
  return out
}

async function exchange(instance: Instance, method: string, url: URL, headers: readonly (readonly [string, string])[], body: Uint8Array): Promise<Resp> {
  let response
  try {
    response = await send({
      method,
      url,
      headers: requestHeaders(instance, headers),
      body: method !== 'GET' && method !== 'HEAD' ? body : null,
      connectTimeoutMs: 10_000,
      totalTimeoutMs: 600_000,
      agent: instance.agent ?? (url.protocol === 'https:' ? defaultAgent() : undefined),
    })
  } catch {
    return errorResp(502, 'UNAVAILABLE', 'the server could not be reached')
  }
  instance.jar.store(response.setCookie, instance.origin.protocol === 'https:', instance.now())
  // 304 answers the conditional request the proxy passes on; it is no redirect.
  if (response.status >= 300 && response.status < 400 && response.status !== 304) {
    response.destroy()
    return errorResp(502, 'UNAVAILABLE', 'the server answered with a redirect')
  }
  const announcesLimit = url.pathname === '/api/permissions' && method === 'GET'
  return answer(instance, method, response, announcesLimit)
}

/** Reads the instance's answer within the memory it may borrow and turns it into what the page receives. */
async function answer(instance: Instance, method: string, response: HttpResponse, announcesLimit: boolean): Promise<Resp> {
  const status = response.status
  const responseCap = instance.limits.responseCap()
  // A HEAD answer announces the length of a body it does not carry: nothing to size or borrow memory for.
  const announced = method === 'HEAD' ? null : response.contentLength
  if (announced !== null && announced > responseCap) {
    response.destroy()
    return errorResp(502, 'UNAVAILABLE', "the server's answer is too large for this client")
  }
  // A large answer borrows its memory first; one of unknown length is held to
  // the small-body size.
  const [reserve, readCap] = announced !== null ? [announced, responseCap] : [0, SMALL_BODY]
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
    return { status, headers: answerHeaders(response), body: method === 'HEAD' ? new Uint8Array() : bytes }
  } finally {
    answerMemory()
  }
}

/** The headers the page may see of the instance's answer; a type that could be a document in the app origin becomes a download. */
function answerHeaders(response: HttpResponse): [string, string][] {
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
  return forwarded
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
