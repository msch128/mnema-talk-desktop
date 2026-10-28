import assert from 'node:assert/strict'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { afterEach, test } from 'node:test'
import { WebSocketServer } from 'ws'
import { Jar } from '../src/core/cookies.ts'
import { Limits } from '../src/core/proxy.ts'
import { MAX_FRAME, MAX_OUTGOING, MAX_SOCKETS, Sockets, socketRequest, socketUrl, type WsEvent } from '../src/core/sockets.ts'

const open: { server: Server, wss: WebSocketServer }[] = []
afterEach(async () => {
  for (const { server, wss } of open.splice(0)) {
    wss.close()
    await new Promise<void>((resolve) => { server.closeAllConnections(); server.close(() => resolve()) })
  }
})

async function start(onConnection: (socket: import('ws').WebSocket, headers: import('node:http').IncomingHttpHeaders) => void) {
  const server = createServer()
  const wss = new WebSocketServer({ server, maxPayload: 4 << 20 })
  open.push({ server, wss })
  wss.on('connection', (socket, request) => onConnection(socket, request.headers))
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const origin = new URL(`http://127.0.0.1:${(server.address() as AddressInfo).port}`)
  const jar = new Jar(null, null, 0)
  jar.store(['session=s3cret; Max-Age=3600'], false, Math.floor(Date.now() / 1000))
  return { origin, jar, limits: new Limits(), now: () => Math.floor(Date.now() / 1000) }
}

function events(): { list: WsEvent[], emit: (e: WsEvent) => boolean, until: (kind: string, count?: number) => Promise<void> } {
  const list: WsEvent[] = []
  const waiters: (() => void)[] = []
  return {
    list,
    emit: (e) => { list.push(e); waiters.splice(0).forEach((w) => w()); return true },
    async until(kind, count = 1) {
      while (list.filter((e) => e.kind === kind).length < count) await new Promise<void>((resolve) => waiters.push(resolve))
    },
  }
}

test('the host opens /api/ws with the instance origin and the cookie, and relays text both ways', async () => {
  let seen: import('node:http').IncomingHttpHeaders = {}
  const instance = await start((socket, headers) => {
    seen = headers
    socket.on('message', (data, isBinary) => { if (!isBinary) socket.send(`echo:${data.toString()}`) })
    socket.send(Buffer.from([1, 2, 3]), { binary: true })
    socket.send('hello')
  })
  const sockets = new Sockets()
  const ev = events()
  const id = sockets.connect(instance, false, ev.emit)
  await ev.until('message')
  assert.equal(seen.origin, instance.origin.origin)
  assert.equal(seen.cookie, 'session=s3cret')
  assert.deepEqual(ev.list.map((e) => e.kind), ['open', 'message'])
  assert.deepEqual(ev.list[1], { kind: 'message', id, data: 'hello' })
  sockets.send(id, 'ping')
  await ev.until('message', 2)
  assert.deepEqual(ev.list[2], { kind: 'message', id, data: 'echo:ping' })
  sockets.close(id)
  await ev.until('close')
  assert.equal(ev.list.filter((e) => e.kind === 'close').length, 1)
  assert.equal(sockets.count, 0)
  assert.throws(() => sockets.send(id, 'late'), /unknown socket/)
})

test('an oversized frame ends the socket and the number of sockets is bounded', async () => {
  const instance = await start((socket) => { socket.send('x'.repeat(MAX_FRAME + 1)) })
  const sockets = new Sockets()
  const ev = events()
  sockets.connect(instance, false, ev.emit)
  await ev.until('close')
  assert.ok(!ev.list.some((e) => e.kind === 'message'))

  const quiet = await start(() => {})
  const held = new Sockets()
  const ids: number[] = []
  for (let i = 0; i < MAX_SOCKETS; i++) ids.push(held.connect(quiet, false, () => true))
  assert.throws(() => held.connect(quiet, false, () => true), /too many sockets/)
  held.closeAll()
})

test('a refused connection reports close once and never open', async () => {
  const instance = await start(() => {})
  const stale = open[open.length - 1]
  await new Promise<void>((resolve) => { stale?.server.closeAllConnections(); stale?.server.close(() => resolve()) })
  const ev = events()
  new Sockets().connect(instance, false, ev.emit)
  await ev.until('close')
  assert.deepEqual(ev.list.map((e) => e.kind), ['close'])
})

test('outgoing messages are bounded', async () => {
  const instance = await start(() => {})
  const sockets = new Sockets()
  const ev = events()
  const id = sockets.connect(instance, false, ev.emit)
  await ev.until('open')
  assert.throws(() => sockets.send(id, 'x'.repeat((1 << 20) + 1)), /message too large/)
  sockets.close(id)
  await ev.until('close')
})

test('a stalled instance cannot make the host queue frames without bound', async () => {
  let stalled: import('ws').WebSocket | null = null
  const instance = await start((socket) => {
    // The instance stops reading: its side of the connection is paused.
    stalled = socket
    ;(socket as unknown as { _socket: { pause(): void } })._socket.pause()
  })
  const sockets = new Sockets()
  const ev = events()
  const id = sockets.connect(instance, false, ev.emit)
  await ev.until('open')
  const frame = 'x'.repeat(MAX_OUTGOING)
  let sent = 0
  assert.throws(() => { for (; sent < 2000; sent++) sockets.send(id, frame) }, /backed up/)
  assert.ok(sent > 0 && sent < 2000, `refused after ${sent} frames`)
  // The paused peer would never answer a close handshake.
  ;(stalled as import('ws').WebSocket | null)?.terminate()
  await ev.until('close')
})

test('a page may ask for the plain socket or the one with conversations and nothing else', () => {
  assert.equal(socketRequest('/api/ws'), false)
  assert.equal(socketRequest('/api/ws?dm=1'), true)
  for (const refused of ['/api/ws?dm=0', '/api/ws?dm=1&x=1', '/api/ws?x=1', '/api/ws?', '/api/ws/', '/api/ws#x', '/api/other', '/api/ws?dm=1\n', '//evil.example/api/ws', '']) {
    assert.throws(() => socketRequest(refused), /Only \/api\/ws can be opened\./, JSON.stringify(refused))
  }
})

test('the conversation question reaches the server only when asked', async () => {
  const urls: string[] = []
  const server = createServer()
  const wss = new WebSocketServer({ server })
  open.push({ server, wss })
  wss.on('connection', (socket, request) => { urls.push(request.url ?? ''); socket.send('hello') })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const origin = new URL(`http://127.0.0.1:${(server.address() as AddressInfo).port}`)
  const instance = { origin, jar: new Jar(null, null, 0), limits: new Limits(), now: () => 0 }
  assert.equal(socketUrl(instance).search, '')
  assert.equal(socketUrl(instance, true).search, '?dm=1')
  const sockets = new Sockets()
  for (const dm of [false, true]) {
    const ev = events()
    sockets.connect(instance, dm, ev.emit)
    await ev.until('message')
  }
  assert.deepEqual(urls, ['/api/ws', '/api/ws?dm=1'])
  sockets.closeAll()
})
