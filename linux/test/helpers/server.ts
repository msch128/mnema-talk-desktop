import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

export interface Seen { method: string, url: string, headers: IncomingMessage['headers'], body: Buffer }

export interface TestServer {
  origin: URL
  requests: Seen[]
  close(): Promise<void>
}

export type Handler = (request: Seen, response: ServerResponse) => void

/** A loopback HTTP server for one test. `handler` answers; every request is recorded. */
export async function serve(handler: Handler): Promise<TestServer> {
  const requests: Seen[] = []
  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = []
    req.on('data', (c: Buffer) => chunks.push(c))
    req.on('end', () => {
      const seen: Seen = { method: req.method ?? '', url: req.url ?? '', headers: req.headers, body: Buffer.concat(chunks) }
      requests.push(seen)
      handler(seen, res)
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address() as AddressInfo
  return {
    origin: new URL(`http://127.0.0.1:${port}`),
    requests,
    close: () => new Promise((resolve) => { server.closeAllConnections(); server.close(() => resolve()) }),
  }
}

export function reply(status: number, headers: Record<string, string | string[]>, body: string | Buffer): Handler {
  return (_request, res) => {
    res.writeHead(status, { 'content-length': String(Buffer.byteLength(body)), ...headers })
    res.end(body)
  }
}
