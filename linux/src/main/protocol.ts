/**
 * The `mnema://localhost` origin. Each kind of window gets its own session and
 * therefore its own handler, so what a window may fetch does not depend on a
 * label it could claim: the selector and the instance window get the verified
 * bundle (the instance window also the proxy to its instance), the attachment
 * window gets the proxy only, the update and picker windows get the host's own
 * pages. A port of `handle` in `experimental/app/src/web_client.rs`.
 */
import { type Resp, forward, payloadTooLarge, plain, serveBundle } from '../core/proxy.ts'
import { refusalText } from '../core/store.ts'
import type { Session } from '../core/instances.ts'
import type { Trusted } from '../core/trusted.ts'
import { APP_HOST, APP_SCHEME } from '../core/origin.ts'

/** Statuses a `Response` may not carry a body for. */
const NO_BODY = new Set([101, 204, 205, 304])

export function toResponse(resp: Resp): Response {
  const headers = new Headers()
  for (const [name, value] of resp.headers) {
    try { headers.append(name, value) } catch { /* an invalid header is dropped, not the response */ }
  }
  const status = resp.status >= 200 && resp.status <= 599 ? resp.status : 500
  return new Response(NO_BODY.has(status) ? null : Buffer.from(resp.body), { status, headers })
}

/** A page of the host itself, not of the web bundle. */
export interface Asset { type: string, body: Uint8Array }

export type RequestHandler = (request: Request) => Promise<Response>

/**
 * Reads a request body up to `cap` bytes. `null` when it is longer: the
 * declared length is checked first, and a body that lies is cut off.
 */
async function readRequestBody(request: Request, cap: number): Promise<Uint8Array | null> {
  const declared = Number(request.headers.get('content-length'))
  if (Number.isFinite(declared) && declared > cap) return null
  if (!request.body) return new Uint8Array()
  const reader = request.body.getReader()
  const chunks: Uint8Array[] = []
  let total = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    total += value.length
    if (total > cap) {
      await reader.cancel().catch(() => undefined)
      return null
    }
    chunks.push(value)
  }
  return Buffer.concat(chunks, total)
}

function ours(url: URL): boolean {
  return url.protocol === `${APP_SCHEME}:` && url.hostname === APP_HOST && url.port === '' && url.username === '' && url.password === ''
}

function isApi(path: string): boolean {
  return path === '/api' || path.startsWith('/api/')
}

function bundlePage(trusted: Trusted, method: string, path: string): Resp {
  const held = trusted.view()
  if (!held) return plain(503, refusalText(trusted.store.emptyReason()))
  return serveBundle(held.bundle, method, path)
}

/**
 * Why a server's page shows nothing but a sentence: it is still being checked,
 * or it was refused (too old or too new for any verified bundle, unreachable,
 * its bundle withdrawn). Shown in that server's tab only.
 */
function unavailable(trusted: Trusted, server: string): Resp {
  const phase = trusted.registry.slot(server)?.phase
  if (phase?.kind === 'starting') return plain(503, 'Connecting to the server.')
  if (phase?.kind === 'refused') return plain(503, phase.why)
  return plain(503, 'No instance is selected.')
}

/** The proxy: `/api/...` of the server this page belongs to, with that server's cookies. */
async function proxy(session: Session, request: Request, url: URL): Promise<Resp> {
  const headers: [string, string][] = [...request.headers.entries()]
  const method = request.method
  let body: Uint8Array = new Uint8Array()
  if (method !== 'GET' && method !== 'HEAD') {
    const cap = session.instance.limits.requestCap(url.pathname)
    const read = await readRequestBody(request, cap)
    if (!read) return payloadTooLarge(cap)
    body = read
  }
  return forward(session.instance, method, url.pathname + url.search, headers, body)
}

export type WindowKind = 'selector' | 'instance' | 'attachment'

/**
 * The handler of one kind of page. `server` binds the handler of a server's
 * page (or attachment window) to that server: the session it uses is looked up
 * by this id on every request, so a removed or restarted server is never
 * reached through an old page.
 */
export function appHandler(kind: WindowKind, trusted: Trusted, debug = false, server = ''): RequestHandler {
  return async (request) => {
    let resp: Resp
    try {
      const url = new URL(request.url)
      const session = kind === 'selector' ? null : trusted.session(server)
      if (!ours(url)) {
        resp = plain(403, 'Forbidden.')
      } else if (kind === 'attachment') {
        resp = !isApi(url.pathname) ? plain(403, 'Forbidden.') : session ? await proxy(session, request, url) : unavailable(trusted, server)
      } else if (isApi(url.pathname)) {
        resp = kind === 'instance' ? (session ? await proxy(session, request, url) : unavailable(trusted, server)) : plain(404, 'Not found.')
      } else if (kind === 'instance') {
        resp = session ? serveBundle(session.held.bundle, request.method, url.pathname) : unavailable(trusted, server)
      } else {
        resp = bundlePage(trusted, request.method, url.pathname)
      }
      if (debug) console.error(`[proto] ${kind} ${request.url} -> ${resp.status}`)
    } catch {
      resp = plain(500, 'The request could not be answered.')
    }
    return toResponse(resp)
  }
}

/** Serves fixed pages of the host (update window, screen picker) under a prefix. */
export function assetHandler(prefix: string, files: ReadonlyMap<string, Asset>, csp: string): RequestHandler {
  return (request) => {
    let resp: Resp
    const url = (() => { try { return new URL(request.url) } catch { return null } })()
    const asset = url && ours(url) && url.pathname.startsWith(prefix) ? files.get(url.pathname.slice(prefix.length) || 'index.html') : undefined
    if (!asset || (request.method !== 'GET' && request.method !== 'HEAD')) {
      resp = plain(404, 'Not found.')
    } else {
      resp = {
        status: 200,
        headers: [['content-type', asset.type], ['content-security-policy', csp], ['x-content-type-options', 'nosniff'], ['cache-control', 'no-store']],
        body: request.method === 'HEAD' ? new Uint8Array() : asset.body,
      }
    }
    return Promise.resolve(toResponse(resp))
  }
}

/** The policy of the host's own pages: their script and style only, no network. */
export const HOST_PAGE_CSP = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
