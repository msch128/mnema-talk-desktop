/**
 * A server on a port of its own, reached through a pasted invite link, and a
 * server that restarts on that port: the host keeps the session and opens a
 * new WebSocket as soon as the server is back. The server listens on every
 * address (loopback IPv4 and IPv6), so the name `localhost` reaches it either way.
 */
import assert from 'node:assert/strict'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { after, before, test } from 'node:test'
import { WebSocketServer } from 'ws'
import type { Page } from './cdp.ts'
import { type Running, launch } from './launch.ts'

const generation = { value: 0 }
const upgrades: { cookie?: string, origin?: string }[] = []
let server: Server
let wss: WebSocketServer
let port = 0
let origin = ''
let app: Running
let selector: Page
let instance: Page

function start(listenPort: number): Promise<Server> {
  const created = createServer((req, res) => {
    const json = (status: number, body: unknown, headers: Record<string, string> = {}) => { res.writeHead(status, { 'content-type': 'application/json', ...headers }); res.end(JSON.stringify(body)) }
    if (req.url === '/api/health') return json(200, { status: 'ok', version: '2.9.19', web_client_api: 1 })
    if (req.url === '/api/login') return json(200, { ok: true }, { 'set-cookie': 'session=s3cret; Max-Age=3600; Path=/' })
    if (req.url === '/api/echo') return json(200, { generation: generation.value, cookie: req.headers.cookie ?? null })
    return json(401, { error: { code: 'UNAUTHORIZED', message: 'no' } })
  })
  created.on('upgrade', (req, socket, head) => {
    if (req.url !== '/api/ws') { socket.destroy(); return }
    wss.handleUpgrade(req, socket, head, (ws) => {
      upgrades.push({ cookie: req.headers.cookie, origin: req.headers.origin })
      ws.send(`hello:${generation.value}`)
    })
  })
  return new Promise((resolve, reject) => {
    created.once('error', reject)
    created.listen(listenPort, () => resolve(created))
  })
}

async function stop(): Promise<void> {
  // An upgraded connection is no longer the HTTP server's: the sockets are ended by hand.
  for (const client of wss.clients) client.terminate()
  server.closeAllConnections()
  await new Promise<void>((resolve) => server.close(() => resolve()))
}

async function waitFor(check: () => Promise<boolean>, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
}

const echo = () => instance.evaluate<{ generation: number, cookie: string | null }>("fetch('/api/echo').then((r) => r.json())")

/** Opens /api/ws through the host and reports the messages it receives and whether it was closed. */
const openSocket = () => instance.evaluate<number>(`(async () => {
  window.__events = []
  const channel = window.__TAURI_INTERNALS__.transformCallback((m) => window.__events.push(m.message ? m.message : m))
  return window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/ws', onEvent: { id: channel } })
})()`)
const events = () => instance.evaluate<{ kind?: string, data?: string }[]>('window.__events')

before(async () => {
  wss = new WebSocketServer({ noServer: true })
  server = await start(0)
  port = (server.address() as AddressInfo).port
  origin = `http://localhost:${port}`
  app = await launch()
  selector = await app.role('selector')
})

after(async () => {
  await app?.stop()
  wss?.close()
  server?.closeAllConnections()
  server?.close()
})

test('a pasted invite link with a port opens the server on that port', async () => {
  const link = `${origin}/invite/abc234xyz789?x=1`
  assert.equal(await selector.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_open_instance', { address: '${link}' }).then(() => 'ok', (e) => String(e))`), 'ok')
  instance = await app.role('instance')
  await instance.watch()
  assert.equal(await instance.evaluate('location.origin'), 'mnema://localhost')
  assert.equal(await instance.evaluate("fetch('/api/health').then((r) => r.status)"), 200)
})

test('sign-in and the WebSocket work on the custom port, and the host sends the origin with the port', async () => {
  assert.equal(await instance.evaluate("fetch('/api/login', { method: 'POST' }).then((r) => r.status)"), 200)
  assert.deepEqual(await echo(), { generation: 0, cookie: 'session=s3cret' })
  await openSocket()
  await waitFor(async () => (await events()).some((e) => e.data === 'hello:0'))
  assert.deepEqual(upgrades.at(-1), { cookie: 'session=s3cret', origin })
})

test('the server restarts on the same port: the socket ends, requests fail meanwhile, and everything works again', async () => {
  await stop()
  await waitFor(async () => (await events()).some((e) => e.kind === 'close'))
  assert.equal(await instance.evaluate("fetch('/api/echo').then((r) => r.ok ? 'reached' : 'down', () => 'down')"), 'down')
  generation.value = 1
  server = await start(port)
  await waitFor(async () => (await instance.evaluate<string>("fetch('/api/echo').then((r) => r.ok ? 'reached' : 'down', () => 'down')")) === 'reached')
  assert.deepEqual(await echo(), { generation: 1, cookie: 'session=s3cret' }, 'the session cookie survives the restart')
  await openSocket()
  await waitFor(async () => (await events()).some((e) => e.data === 'hello:1'))
  assert.deepEqual(upgrades.at(-1), { cookie: 'session=s3cret', origin })
})
