/**
 * End to end, in a real Electron: the signed bundle (`bundle/builtin.zip`, the
 * one the build embeds) is verified and served, an instance is opened through
 * the host, and what the page may and may not do is checked from inside the
 * page. Needs the Electron binary and a display (xvfb on a headless Linux).
 *
 *   npm run e2e
 */
import assert from 'node:assert/strict'
import { type IncomingHttpHeaders, createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { after, before, test } from 'node:test'
import { WebSocketServer } from 'ws'
import type { Page } from './cdp.ts'
import { type Running, launch } from './launch.ts'

let server: Server
let wss: WebSocketServer
let address = ''
const seen: { path: string, headers: IncomingHttpHeaders }[] = []
const sockets: { origin?: string, cookie?: string, messages: string[], url?: string }[] = []
let app: Running
let selector: Page
let instance: Page

before(async () => {
  server = createServer((req, res) => {
    seen.push({ path: req.url ?? '', headers: req.headers })
    const json = (status: number, body: unknown, headers: Record<string, string | string[]> = {}) => {
      res.writeHead(status, { 'content-type': 'application/json', ...headers })
      res.end(JSON.stringify(body))
    }
    if (req.url === '/api/health') return json(200, { status: 'ok', version: '0.8.2', web_client_api: 1 })
    if (req.url === '/api/echo') return json(200, { origin: req.headers.origin ?? null, cookie: req.headers.cookie ?? null, referer: req.headers.referer ?? null })
    if (req.url === '/api/login') return json(200, { ok: true }, { 'set-cookie': 'session=s3cret; Max-Age=3600; Path=/' })
    if (req.url === '/api/html') { res.writeHead(200, { 'content-type': 'text/html' }); res.end('<script>alert(1)</script>'); return }
    if (req.url === '/api/permissions') return json(200, { owner: false, max_upload_mb: 40 })
    return json(401, { error: { code: 'UNAUTHORIZED', message: 'no' } })
  })
  wss = new WebSocketServer({ noServer: true })
  server.on('upgrade', (req, socket, head) => {
    if (req.url !== '/api/ws' && req.url !== '/api/ws?dm=1') { socket.destroy(); return }
    wss.handleUpgrade(req, socket, head, (ws) => {
      const record: (typeof sockets)[number] = { origin: req.headers.origin, cookie: req.headers.cookie, messages: [], url: req.url }
      sockets.push(record)
      ws.on('message', (data) => { record.messages.push(data.toString()); ws.send(`echo:${data.toString()}`) })
      ws.send('hello')
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  address = `http://127.0.0.1:${(server.address() as AddressInfo).port}`
  app = await launch()
  selector = await app.role('selector')
  await selector.watch()
})

after(async () => {
  await app?.stop()
  wss?.close()
  server?.closeAllConnections()
  server?.close()
})

test('the selector runs the verified bundle in a secure context with what the end-to-end encryption needs', async () => {
  const identity = await selector.evaluate<{ version: string, source: string, webClientApi: number, keyId: string, role: string }>('JSON.parse(JSON.stringify(window.__MNEMA_TRUSTED_APP__))')
  assert.match(identity.version, /^\d+\.\d+\.\d+$/)
  assert.deepEqual([identity.source, identity.webClientApi, identity.keyId, identity.role], ['builtin', 1, '1069c76ff2e11272', 'selector'])
  assert.equal(await selector.evaluate('window.isTauri'), true)
  assert.equal(await selector.evaluate('isSecureContext && typeof crypto.subtle === "object" && location.origin'), 'mnema://localhost')
  // The fail-closed check of the app's media guard (web/src/lib/media/guard.ts).
  assert.equal(await selector.evaluate("typeof Worker === 'function' && typeof RTCRtpScriptTransform === 'function' && typeof MessageChannel === 'function'"), true)
  assert.equal(await selector.evaluate("typeof RTCPeerConnection === 'function' && typeof WebAssembly === 'object' && typeof indexedDB === 'object'"), true)
  // Nothing of Node or Electron is reachable.
  assert.equal(await selector.evaluate("typeof require + typeof process + typeof module + typeof ipcRenderer + typeof electron"), 'undefinedundefinedundefinedundefinedundefined')
  const index = await selector.evaluate<{ csp: string, type: string }>("fetch('/index.html').then((r) => ({ csp: r.headers.get('content-security-policy'), type: r.headers.get('content-type') }))")
  assert.match(index.csp, /script-src 'self' 'wasm-unsafe-eval'/)
  assert.match(index.type, /text\/html/)
})

test('a page never reaches the network itself', async () => {
  const probes = [`fetch('${address}/api/health').then(() => 'reached', () => 'blocked')`, "fetch('https://example.invalid/').then(() => 'reached', () => 'blocked')",
    `new Promise((resolve) => { const s = new WebSocket('${address.replace('http', 'ws')}/api/ws'); s.onopen = () => resolve('reached'); s.onerror = () => resolve('blocked') })`]
  for (const probe of probes) assert.equal(await selector.evaluate(probe), 'blocked', probe)
  assert.equal(seen.filter((s) => s.path === '/api/ws').length, 0)
  // The selector has no instance to talk to.
  assert.equal(await selector.evaluate("fetch('/api/health').then((r) => r.status)"), 404)
})

test('commands are refused to windows that may not call them', async () => {
  const call = (page: Page, command: string, args = '{}') => page.evaluate<string>(`window.__TAURI_INTERNALS__.invoke('${command}', ${args}).then(() => 'ok', (e) => String(e))`)
  assert.equal(await call(selector, 'native_update_state'), 'Not allowed.')
  assert.equal(await call(selector, 'desktop_ws_send', '{ id: 1, data: "x" }'), 'Not allowed.')
  assert.equal(await call(selector, 'desktop_gaming_sync'), 'Unknown command.')
  assert.equal(await call(selector, 'desktop_open_instance', '{ address: "https://user@community.example" }'), 'Use an HTTPS origin without credentials, a path or query.')
  assert.equal(await call(selector, 'desktop_open_instance', '{ address: "http://127.0.0.1:9" }'), 'The instance refused the connection. Check the address and the port.')
  assert.equal(await call(selector, 'desktop_open_instance', '{}'), 'invalid arguments')
})

test('an instance is opened through the host and its pages run the same verified bundle', async () => {
  assert.equal(await selector.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_open_instance', { address: '${address}' }).then(() => 'ok', (e) => String(e))`), 'ok')
  // The selector is hidden; the page of the server is the one that runs the bundle in the instance role.
  instance = await app.role('instance')
  await instance.watch()
  const identity = await instance.evaluate<{ role: string, keyId: string }>('JSON.parse(JSON.stringify(window.__MNEMA_TRUSTED_APP__))')
  assert.deepEqual([identity.role, identity.keyId], ['instance', '1069c76ff2e11272'])
  await waitFor(() => instance.evaluate<boolean>("document.body.innerText.includes('Sign in')"))
  assert.equal(await instance.evaluate("fetch('/api/health').then((r) => r.status)"), 200)
  assert.deepEqual(instance.problems.filter((p) => /Content Security Policy|Refused to/i.test(p)), [])
})

test('the host speaks to the instance as a browser on its own origin, and keeps the session cookie to itself', async () => {
  const echo = () => instance.evaluate<{ origin: string | null, cookie: string | null, referer: string | null }>("fetch('/api/echo', { headers: { 'X-Request-Id': 'r1' } }).then((r) => r.json())")
  const before = await echo()
  assert.deepEqual(before, { origin: address, cookie: null, referer: null })
  assert.equal(await instance.evaluate("fetch('/api/login', { method: 'POST' }).then((r) => r.headers.get('set-cookie'))"), null)
  assert.equal(await instance.evaluate('document.cookie'), '')
  assert.deepEqual(await echo(), { origin: address, cookie: 'session=s3cret', referer: null })
  assert.equal(seen.filter((s) => s.path === '/api/echo').at(-1)?.headers['x-request-id'], 'r1')
})

test('an answer that could run as a document becomes a sandboxed download', async () => {
  const answer = await instance.evaluate<{ type: string, disposition: string, csp: string, sniff: string }>(
    "fetch('/api/html').then((r) => ({ type: r.headers.get('content-type'), disposition: r.headers.get('content-disposition'), csp: r.headers.get('content-security-policy'), sniff: r.headers.get('x-content-type-options') }))")
  assert.deepEqual(answer, { type: 'application/octet-stream', disposition: 'attachment', csp: "sandbox; default-src 'none'", sniff: 'nosniff' })
})

test('the WebSocket of the instance is opened by the host with its origin and cookie', async () => {
  const result = await instance.evaluate<{ events: { kind: string, data?: string }[] }>(`(async () => {
    const events = []
    const channel = window.__TAURI_INTERNALS__.transformCallback((m) => events.push(m))
    const id = await window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/ws', onEvent: { id: channel } })
    const until = async (n) => { for (let i = 0; i < 100 && events.length < n; i++) await new Promise((r) => setTimeout(r, 50)) }
    await until(2)
    await window.__TAURI_INTERNALS__.invoke('desktop_ws_send', { id, data: 'ping' })
    await until(3)
    await window.__TAURI_INTERNALS__.invoke('desktop_ws_close', { id })
    await until(5)
    return { events: events.map((m) => m.message ? { index: m.index, ...m.message } : m) }
  })()`)
  const kinds = result.events.map((e) => e.kind ?? 'end')
  assert.deepEqual(kinds, ['open', 'message', 'message', 'close', 'end'])
  assert.deepEqual(result.events.filter((e) => e.kind === 'message').map((e) => e.data), ['hello', 'echo:ping'])
  assert.deepEqual([sockets[0]?.origin, sockets[0]?.cookie, sockets[0]?.messages], [address, 'session=s3cret', ['ping']])
  assert.equal(await instance.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/other', onEvent: { id: 1 } }).then(() => 'ok', (e) => String(e))`), 'Only /api/ws can be opened.')
  assert.equal(await instance.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/ws?x=1', onEvent: { id: 1 } }).then(() => 'ok', (e) => String(e))`), 'Only /api/ws can be opened.')
})

test('the conversation question is relayed to the server and nothing else of the query', async () => {
  const before = sockets.length
  const id = await instance.evaluate<number>(`window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/ws?dm=1', onEvent: { id: window.__TAURI_INTERNALS__.transformCallback(() => {}) } })`)
  await waitFor(async () => sockets.length > before)
  assert.equal(sockets.at(-1)?.url, '/api/ws?dm=1')
  await instance.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_ws_close', { id: ${id} })`)
  const identity = await instance.evaluate<string[]>('[...window.__MNEMA_TRUSTED_APP__.capabilities]')
  assert.deepEqual(identity, ['notify', 'unread', 'ws_query'])
})

test('the rail is a page of the host that no server page can reach or imitate', async () => {
  const rail = await app.role('rail')
  assert.equal(await rail.evaluate("typeof window.__TAURI_INTERNALS__ + typeof window.__MNEMA_TRUSTED_APP__"), 'undefinedundefined')
  assert.equal(await rail.evaluate("fetch('/api/health').then((r) => r.status, () => 'blocked')"), 'blocked')
  assert.equal(await rail.evaluate("document.querySelectorAll('.tile').length"), 1)
  // Its policy: no network at all.
  assert.equal(await rail.evaluate("fetch('mnema://localhost/rail.js').then(() => 'reached', () => 'blocked')"), 'blocked')
  // A server page and the selector cannot name the rail's commands, even though they have the same origin.
  for (const page of [instance, selector]) {
    assert.equal(await page.evaluate("window.__TAURI_INTERNALS__.invoke('rail_remove', { id: '0000000000000000' }).then(() => 'ok', (e) => String(e))"), 'Unknown command.')
    assert.equal(await page.evaluate('typeof window.__MNEMA_RAIL_HOST__'), 'undefined')
  }
  rail.close()
})

test('only what the app uses is permitted, and only to the instance window', async () => {
  assert.equal(await instance.evaluate('Notification.requestPermission()'), 'granted')
  assert.equal(await instance.evaluate("new Promise((resolve) => navigator.geolocation.getCurrentPosition(() => resolve('granted'), (e) => resolve(e.code === 1 ? 'denied' : 'error ' + e.code)))"), 'denied')
  assert.equal(await selector.evaluate('Notification.requestPermission()'), 'denied')
  // No device ever: the permissions policy of the page and the session both say no.
  assert.equal(await instance.evaluate("navigator.usb === undefined || navigator.usb.getDevices().then((d) => d.length, () => 'refused')"), 'refused')
})

test('navigation stays inside the verified app', async () => {
  await instance.evaluate("setTimeout(() => { location.href = 'mnema://localhost/other.html' }, 0)")
  await new Promise((resolve) => setTimeout(resolve, 1000))
  assert.equal(await instance.evaluate('location.pathname + document.body.innerText.includes("Sign in")'), '/true')
  assert.equal(await instance.evaluate("window.open('about:blank') === null"), true)
})

async function waitFor(check: () => Promise<boolean>, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
}
