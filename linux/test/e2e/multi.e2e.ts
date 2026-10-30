/**
 * End to end, in a real Electron: two servers side by side. Each has its own
 * page, its own session and its own browser storage; nothing of one reaches the
 * other. Sign in to both, a report from the page in the background raises its
 * own count on the rail while the other is in front, switching works, and
 * removing a server wipes it (its stored session, its storage, the list).
 */
import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { type IncomingHttpHeaders, createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'
import { after, before, test } from 'node:test'
import { WebSocketServer } from 'ws'
import type { Page } from './cdp.ts'
import { type Running, launch } from './launch.ts'

interface Fake { name: string, server: Server, wss: WebSocketServer, address: string, seen: { path: string, headers: IncomingHttpHeaders }[], upgrades: { url?: string, cookie?: string, origin?: string }[] }

async function fake(name: string): Promise<Fake> {
  const seen: Fake['seen'] = []
  const upgrades: Fake['upgrades'] = []
  const server = createServer((req, res) => {
    seen.push({ path: req.url ?? '', headers: req.headers })
    const json = (status: number, body: unknown, headers: Record<string, string> = {}) => { res.writeHead(status, { 'content-type': 'application/json', ...headers }); res.end(JSON.stringify(body)) }
    if (req.url === '/api/health') return json(200, { status: 'ok', version: '1.1.0', web_client_api: 1 })
    if (req.url === '/api/login') return json(200, { ok: true }, { 'set-cookie': `session=${name}-token; Max-Age=3600; Path=/` })
    if (req.url === '/api/echo') return json(200, { server: name, cookie: req.headers.cookie ?? null })
    return json(401, { error: { code: 'UNAUTHORIZED', message: 'no' } })
  })
  const wss = new WebSocketServer({ noServer: true })
  server.on('upgrade', (req, socket, head) => {
    upgrades.push({ url: req.url, cookie: req.headers.cookie, origin: req.headers.origin })
    wss.handleUpgrade(req, socket, head, (ws) => ws.send('hello'))
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  return { name, server, wss, address: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, seen, upgrades }
}

let a: Fake
let b: Fake
let app: Running
let selector: Page
let rail: Page
let pageA: Page
let pageB: Page

const invoke = (page: Page, command: string, args = '{}') => page.evaluate<string>(`window.__TAURI_INTERNALS__.invoke('${command}', ${args}).then(() => 'ok', (e) => String(e))`)
const railCall = (command: string, args = '{}') => rail.evaluate<unknown>(`window.__MNEMA_RAIL_HOST__.invoke('${command}', ${args}).then((v) => v, (e) => 'ERR ' + String(e))`)
const state = () => railCall('rail_state') as Promise<{ active: string | null, adding: boolean, instances: { id: string, name: string, unread: number, mentions: number, status: string }[] }>
const echo = (page: Page) => page.evaluate<{ server: string, cookie: string | null }>("fetch('/api/echo').then((r) => r.json())")
const idOf = (fakeServer: Fake) => state().then((s) => s.instances.find((i) => i.name === `127.0.0.1:${fakeServer.address.split(':').at(-1)}`)!.id)

async function waitFor(check: () => Promise<boolean>, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error('timed out')
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
}

/** The page of a server is told apart by what its server answers. */
const pageOf = (name: string, timeoutMs?: number) => app.pageWhere(async (page) => (await page.evaluate<string | null>("window.__MNEMA_TRUSTED_APP__?.role === 'instance' ? fetch('/api/echo').then((r) => r.json()).then((j) => j.server) : null")) === name, timeoutMs)

before(async () => {
  a = await fake('A')
  b = await fake('B')
  app = await launch()
  selector = await app.role('selector')
  rail = await app.role('rail')
})

after(async () => {
  await app?.stop()
  for (const f of [a, b]) { f?.wss.close(); f?.server.closeAllConnections(); f?.server.close() }
})

test('two servers are added, the last one is in front, and the same server twice is not two', async () => {
  assert.equal(await invoke(selector, 'desktop_open_instance', `{ address: '${a.address}' }`), 'ok')
  pageA = await pageOf('A')
  // The page for adding a server comes back from the rail.
  await railCall('rail_add')
  assert.equal(((await state()).adding), true)
  assert.equal(await invoke(selector, 'desktop_open_instance', `{ address: '${b.address}' }`), 'ok')
  pageB = await pageOf('B')
  const s = await state()
  assert.deepEqual(s.instances.map((i) => i.status), ['ready', 'ready'])
  assert.equal(s.active, s.instances[1]?.id)
  assert.equal(await invoke(selector, 'desktop_open_instance', `{ address: '${a.address}' }`), 'ok')
  const again = await state()
  assert.equal(again.instances.length, 2)
  assert.equal(again.active, again.instances[0]?.id, 'adding a known server brings it forward')
})

test('browser storage of one server is not visible to the other', async () => {
  await pageA.evaluate("localStorage.setItem('probe', 'A')")
  assert.equal(await pageB.evaluate("localStorage.getItem('probe')"), null)
  await pageB.evaluate("localStorage.setItem('probe', 'B')")
  assert.equal(await pageA.evaluate("localStorage.getItem('probe')"), 'A')
  assert.equal(await pageA.evaluate('document.cookie'), '')
})

test('cookies are held by the host, one jar per server, and each page gets only its own', async () => {
  await pageA.evaluate("fetch('/api/login', { method: 'POST' })")
  assert.deepEqual(await echo(pageA), { server: 'A', cookie: 'session=A-token' })
  assert.deepEqual(await echo(pageB), { server: 'B', cookie: null })
  await pageB.evaluate("fetch('/api/login', { method: 'POST' })")
  assert.deepEqual(await echo(pageB), { server: 'B', cookie: 'session=B-token' })
  assert.deepEqual(await echo(pageA), { server: 'A', cookie: 'session=A-token' })
  assert.ok(!b.seen.some((s) => String(s.headers.cookie).includes('A-token')))
  assert.ok(!a.seen.some((s) => String(s.headers.cookie).includes('B-token')))
})

test('sockets carry their own server\'s cookie, and one server cannot use another\'s socket', async () => {
  const open = (page: Page, path: string) => page.evaluate<number>(`window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '${path}', onEvent: { id: window.__TAURI_INTERNALS__.transformCallback(() => {}) } })`)
  const idA = await open(pageA, '/api/ws?dm=1')
  await waitFor(async () => a.upgrades.length > 0)
  assert.deepEqual(a.upgrades[0], { url: '/api/ws?dm=1', cookie: 'session=A-token', origin: a.address })
  assert.equal(b.upgrades.length, 0)
  assert.equal(await invoke(pageB, 'desktop_ws_send', `{ id: ${idA}, data: 'x' }`), 'unknown socket')
  assert.equal(await invoke(pageA, 'desktop_ws_send', `{ id: ${idA}, data: 'x' }`), 'ok')
  await invoke(pageA, 'desktop_ws_close', `{ id: ${idA} }`)
})

test('a page cannot drive the list and cannot name another server', async () => {
  for (const page of [pageA, pageB, selector]) {
    assert.equal(await invoke(page, 'rail_remove', "{ id: '0000000000000000' }"), 'Unknown command.')
  }
  assert.equal(await invoke(pageA, 'desktop_open_instance', `{ address: '${b.address}' }`), 'Only the instance selector may open an instance.')
  // A server argument is ignored: counts go to the server of the page that sends them.
  assert.equal(await invoke(pageA, 'desktop_report_unread', "{ unread: 1, mentions: 1, id: 'ffffffffffffffff', server: 'x' }"), 'ok')
  const s = await state()
  assert.deepEqual(s.instances.map((i) => [i.unread, i.mentions]), [[1, 1], [0, 0]])
  await invoke(pageA, 'desktop_report_unread', '{ unread: 0, mentions: 0 }')
})

test('a report from the server in the background raises its own count while the other is in front', async () => {
  const s = await state()
  const [first, second] = s.instances
  await railCall('rail_activate', `{ id: '${first!.id}' }`)
  assert.equal((await state()).active, first!.id)
  assert.equal(await invoke(pageB, 'desktop_report_unread', '{ unread: 2, mentions: 3 }'), 'ok')
  const after = await state()
  assert.deepEqual(after.instances.map((i) => [i.unread, i.mentions]), [[0, 0], [2, 3]])
  assert.equal(after.active, first!.id, 'a report does not switch servers')
  await invoke(pageA, 'desktop_report_unread', '{ unread: 0, mentions: 100000 }')
  assert.equal((await state()).instances[0]?.mentions, 999, 'counts are bounded')
  await invoke(pageA, 'desktop_report_unread', '{ unread: 0, mentions: 0 }')
  // An older bundle's plain badge counts as mentions, until the page reports both.
  await invoke(pageA, 'desktop_set_badge', '{ count: 4 }')
  assert.equal((await state()).instances[0]?.mentions, 0, 'a page that reported keeps its report')
  await railCall('rail_activate', `{ id: '${second!.id}' }`)
  assert.equal((await state()).active, second!.id)
})

test('the list is reordered from the rail and written down with addresses only', async () => {
  const before = (await state()).instances.map((i) => i.id)
  await railCall('rail_edit', `{ op: 'step', id: '${before[1]}', delta: -1 }`)
  assert.deepEqual((await state()).instances.map((i) => i.id), [before[1], before[0]])
  const file = join(app.dataDir, 'instances.json')
  const text = readFileSync(file, 'utf8')
  assert.ok(text.includes(a.address) && text.includes(b.address))
  assert.ok(!/cookie|token/i.test(text))
})

test('removing a server wipes its session, its storage and its place, and leaves the other alone', async () => {
  const idB = await idOf(b)
  const sessionsDir = join(app.dataDir, 'sessions')
  assert.ok(existsSync(join(sessionsDir, `${idB}.json`)), 'its session was stored (the file, as there is no secret service here)')
  assert.equal(await railCall('rail_remove', `{ id: '${idB}' }`), null)
  const s = await state()
  assert.equal(s.instances.length, 1)
  assert.equal(s.instances[0]?.name, `127.0.0.1:${a.address.split(':').at(-1)}`)
  assert.equal(s.active, s.instances[0]?.id)
  assert.ok(!existsSync(join(sessionsDir, `${idB}.json`)), 'its stored session is gone')
  await waitFor(async () => !existsSync(join(app.dataDir, 'Partitions', `inst-${idB}`)))
  assert.ok(!readFileSync(join(app.dataDir, 'instances.json'), 'utf8').includes(b.address))
  assert.equal(await pageA.evaluate("localStorage.getItem('probe')"), 'A')
  assert.deepEqual(await echo(pageA), { server: 'A', cookie: 'session=A-token' })
  await assert.rejects(pageOf('B', 3000).then(() => undefined), /timed out|no matching page/, 'its page is gone')
})
