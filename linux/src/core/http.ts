import { type Agent as HttpAgent, type IncomingHttpHeaders, type IncomingMessage, request as httpRequest } from 'node:http'
import { Agent as HttpsAgent, request as httpsRequest } from 'node:https'

/**
 * A thin HTTP client over Node's own stack. It follows no redirect, reads no
 * proxy setting and no cookie jar, and bounds every phase, so each caller sees
 * exactly what the instance answered. Certificates are checked against the
 * system store (see `useSystemCertificates` in the main process).
 */
export interface HttpOptions {
  method: string
  url: URL
  headers?: Record<string, string>
  body?: Uint8Array | null
  connectTimeoutMs: number
  totalTimeoutMs: number
  agent?: HttpAgent
}

export interface HttpResponse {
  status: number
  headers: IncomingHttpHeaders
  setCookie: string[]
  /** The declared `Content-Length`, if the server sent a valid one. */
  contentLength: number | null
  stream: IncomingMessage
  /** Stops the transfer. */
  destroy(): void
}

class HttpError extends Error {}

const sharedHttps = new HttpsAgent({ keepAlive: true, maxSockets: 16, minVersion: 'TLSv1.2' })

export function defaultAgent(): HttpsAgent {
  return sharedHttps
}

export function send(options: HttpOptions): Promise<HttpResponse> {
  const { url } = options
  const secure = url.protocol === 'https:'
  if (!secure && url.protocol !== 'http:') return Promise.reject(new HttpError('unsupported scheme'))
  return new Promise((resolve, reject) => {
    const signal = AbortSignal.timeout(options.totalTimeoutMs)
    const headers: Record<string, string> = { ...options.headers }
    if (options.body && options.body.length > 0) headers['content-length'] = String(options.body.length)
    else if (options.body) headers['content-length'] = '0'
    const req = (secure ? httpsRequest : httpRequest)(url, {
      method: options.method,
      headers,
      signal,
      // Never a system proxy, never a redirect: the caller decides.
      agent: options.agent ?? (secure ? sharedHttps : false),
    }, (res) => {
      const length = Number(res.headers['content-length'])
      const setCookie = res.headers['set-cookie'] ?? []
      resolve({
        status: res.statusCode ?? 0,
        headers: res.headers,
        setCookie,
        contentLength: Number.isSafeInteger(length) && length >= 0 && res.headers['content-length'] !== undefined ? length : null,
        stream: res,
        destroy: () => { res.destroy(); req.destroy() },
      })
    })
    const connect = setTimeout(() => req.destroy(new HttpError('connect timeout')), options.connectTimeoutMs)
    req.once('socket', (socket) => {
      if (!socket.connecting) { clearTimeout(connect); return }
      socket.once(secure ? 'secureConnect' : 'connect', () => clearTimeout(connect))
    })
    req.once('error', (error) => { clearTimeout(connect); reject(error) })
    req.once('close', () => clearTimeout(connect))
    req.end(options.body && options.body.length > 0 ? options.body : undefined)
  })
}

/**
 * Reads at most `cap` bytes. `null` when the body is longer or the transfer
 * broke off; the connection is dropped in that case.
 */
export async function readBody(response: HttpResponse, cap: number, progress?: (received: number, total: number | null) => void): Promise<Buffer | null> {
  const chunks: Buffer[] = []
  let total = 0
  try {
    for await (const chunk of response.stream as AsyncIterable<Buffer>) {
      total += chunk.length
      if (total > cap) {
        response.destroy()
        return null
      }
      chunks.push(chunk)
      progress?.(total, response.contentLength)
    }
  } catch {
    response.destroy()
    return null
  }
  return Buffer.concat(chunks, total)
}

export function headerText(value: string | string[] | undefined): string | undefined {
  return Array.isArray(value) ? value.join(', ') : value
}
