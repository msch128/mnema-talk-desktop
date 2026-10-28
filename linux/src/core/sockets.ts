/**
 * The WebSocket of the selected instance, opened by the host for the app (a
 * port of `experimental/app/src/web_ws.rs`). The page cannot do it itself: its
 * origin is the host's, so the instance would see the wrong `Origin` and no
 * session cookie. Only `/api/ws` can be opened, text frames are relayed both
 * ways, and everything stays bounded.
 */
import WebSocket from 'ws'
import type { Instance } from './proxy.ts'

export const MAX_FRAME = 1 << 20
export const MAX_SOCKETS = 4
export const MAX_OUTGOING = 1 << 20
/** What may wait unsent per socket; a link that stalls cannot make the host queue more. */
export const MAX_BUFFERED = 4 << 20

export type WsEvent =
  | { kind: 'open', id: number }
  | { kind: 'message', id: number, data: string }
  | { kind: 'close', id: number }

/**
 * What a page may ask the host to open: the instance's `/api/ws`, with the one
 * question the server knows, `dm=1` (also send conversation events, 1.1).
 * Anything else, in the path or the query, is refused instead of passed on.
 * Returns whether conversation events were asked for.
 */
export function socketRequest(path: string): boolean {
  if (path === '/api/ws') return false
  if (path === '/api/ws?dm=1') return true
  throw new Error('Only /api/ws can be opened.')
}

export function socketUrl(instance: Instance, dm = false): URL {
  const url = new URL('api/ws', instance.origin)
  if (dm) url.search = 'dm=1'
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url
}

export class Sockets {
  private next = 0
  private readonly open = new Map<number, WebSocket>()

  get count(): number {
    return this.open.size
  }

  /**
   * Starts a connection and returns its id at once; `emit` receives `open` when
   * the instance accepted it, the messages, and `close` exactly once. When
   * `emit` returns false the page is gone and the socket is dropped.
   */
  connect(instance: Instance, dm: boolean, emit: (event: WsEvent) => boolean): number {
    if (this.open.size >= MAX_SOCKETS) throw new Error('too many sockets')
    const id = ++this.next
    const headers: Record<string, string> = { Origin: instance.origin.origin }
    const cookie = instance.jar.header(instance.now())
    if (cookie) headers.Cookie = cookie
    const ws = new WebSocket(socketUrl(instance, dm), {
      headers,
      maxPayload: MAX_FRAME,
      handshakeTimeout: 10_000,
      perMessageDeflate: false,
      followRedirects: false,
      minVersion: 'TLSv1.2',
    })
    this.open.set(id, ws)
    let closed = false
    const finish = () => {
      if (closed) return
      closed = true
      this.open.delete(id)
      emit({ kind: 'close', id })
    }
    ws.on('open', () => {
      if (!emit({ kind: 'open', id })) ws.terminate()
    })
    ws.on('message', (data, isBinary) => {
      // Pings are answered by the library; the app only speaks text.
      if (isBinary) return
      if (!emit({ kind: 'message', id, data: Buffer.isBuffer(data) ? data.toString('utf8') : Buffer.concat(data as Buffer[]).toString('utf8') })) ws.terminate()
    })
    ws.on('error', () => ws.terminate())
    ws.on('close', finish)
    return id
  }

  send(id: number, text: string): void {
    const length = Buffer.byteLength(text)
    if (length > MAX_OUTGOING) throw new Error('message too large')
    const ws = this.open.get(id)
    if (!ws || ws.readyState !== WebSocket.OPEN) throw new Error('unknown socket')
    if (ws.bufferedAmount + length > MAX_BUFFERED) throw new Error('socket is backed up')
    ws.send(text, (error) => { if (error) ws.terminate() })
  }

  close(id: number): void {
    this.open.get(id)?.close()
  }

  closeAll(): void {
    for (const ws of this.open.values()) ws.close()
  }
}
