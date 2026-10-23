// A stand-in for a Mnema instance, for the multi-instance self-test: health,
// a login that sets a cookie named after the server, an echo of what it was
// sent, and a WebSocket that says hello and stays open. Every request and
// upgrade is appended to the log file as one JSON line. No dependencies.
//
//   node scripts/fake-instance.mjs <name> <port> <log-file> [version]
import { createHash } from 'node:crypto'
import { appendFileSync } from 'node:fs'
import { createServer } from 'node:http'

const [name, portText, log, version = '1.1.0'] = process.argv.slice(2)
const port = Number(portText)
const note = (entry) => appendFileSync(log, `${JSON.stringify(entry)}\n`)

const server = createServer((req, res) => {
  note({ kind: 'http', method: req.method, url: req.url, origin: req.headers.origin ?? null, cookie: req.headers.cookie ?? null })
  const json = (status, body, headers = {}) => {
    res.writeHead(status, { 'content-type': 'application/json', ...headers })
    res.end(JSON.stringify(body))
  }
  if (req.url === '/api/health') return json(200, { status: 'ok', version, web_client_api: 1 })
  if (req.url === '/api/login') return json(200, { ok: true }, { 'set-cookie': `session=${name}-token; Max-Age=3600; Path=/` })
  if (req.url === '/api/echo') return json(200, { server: name, cookie: req.headers.cookie ?? null })
  return json(401, { error: { code: 'UNAUTHORIZED', message: 'no' } })
})

server.on('upgrade', (req, socket) => {
  note({ kind: 'upgrade', url: req.url, origin: req.headers.origin ?? null, cookie: req.headers.cookie ?? null })
  const accept = createHash('sha1').update(`${req.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest('base64')
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`)
  const hello = Buffer.from(JSON.stringify({ type: 'hello', server: name }))
  socket.write(Buffer.concat([Buffer.from([0x81, hello.length]), hello]))
  socket.on('error', () => {})
})

server.listen(port, '127.0.0.1', () => note({ kind: 'listening', name, port }))
