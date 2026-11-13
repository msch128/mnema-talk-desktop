/**
 * The Electron client against a REAL mnema-talk server: it adds the server by
 * the address a person types, runs the signed bundle for it, signs in, opens
 * the WebSocket, sends a message and sees it pushed, and follows the server
 * through a restart. The server, its PostgreSQL and S3 storage and the
 * restart belong to scripts/real-server-selftest.mjs (`--run`), which starts
 * this file with the address and the account in the environment; run alone
 * (`npm run e2e` does not include it) the tests are skipped.
 *
 *   node scripts/real-server-selftest.mjs --run <server> xvfb-run -a node --test linux/test/e2e/real/server.e2e.ts
 */
import assert from 'node:assert/strict'
import { appendFileSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, before, test } from 'node:test'
import type { Page } from '../cdp.ts'
import { type Running, launch } from '../launch.ts'

const address = process.env.MNEMA_REAL_SELFTEST_SERVER ?? ''
const user = process.env.MNEMA_REAL_SELFTEST_USER ?? ''
const password = process.env.MNEMA_REAL_SELFTEST_PASSWORD ?? ''
const record = process.env.MNEMA_REAL_SELFTEST ?? ''
const control = process.env.MNEMA_REAL_SELFTEST_CONTROL ?? ''
const skip = address === '' || record === '' || control === ''

let app: Running
let instance: Page
let channel = ''
/** Where the host saves a download in this run (the save dialog is skipped; see `testFolder` in src/main/download.ts). */
const downloads = mkdtempSync(join(tmpdir(), 'mnema-e2e-downloads-'))
const sent: string[] = []

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function waitFor(check: () => Promise<boolean> | boolean, what: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out: ${what}`)
    await sleep(200)
  }
}

const controlHas = (word: string) => existsSync(control) && readFileSync(control, 'utf8').split('\n').includes(word)

/** A request from inside the page, as the web app makes it. */
const api = <T = unknown>(method: string, path: string, body?: unknown) => instance.evaluate<{ status: number, json: T }>(`(async () => {
  const response = await fetch(${JSON.stringify(path)}, { method: ${JSON.stringify(method)}, headers: { 'Content-Type': 'application/json' }, body: ${body === undefined ? 'undefined' : JSON.stringify(JSON.stringify(body))} })
  return { status: response.status, json: await response.json().catch(() => null) }
})()`)

/** Opens /api/ws through the host and collects what it reports. */
const openSocket = () => instance.evaluate<number>(`(async () => {
  window.__events = []
  const channel = window.__TAURI_INTERNALS__.transformCallback((m) => window.__events.push(m.message ? m.message : m))
  return window.__TAURI_INTERNALS__.invoke('desktop_ws_open', { path: '/api/ws', onEvent: { id: channel } })
})()`)
const sawEvent = (test: string) => instance.evaluate<boolean>(`window.__events.some((e) => ${test})`)

async function chat(name: string): Promise<void> {
  await openSocket()
  await waitFor(() => sawEvent("e.kind === 'open'"), `${name}: the server accepts the WebSocket`)
  const text = `selftest-${name}-${Date.now()}`
  const posted = await api('POST', `/api/channels/${channel}/messages`, { content: text, nonce: crypto.randomUUID() })
  assert.ok(posted.status === 200 || posted.status === 201, `status ${posted.status}`)
  sent.push(text)
  await waitFor(() => sawEvent(`e.kind === 'message' && String(e.data).includes(${JSON.stringify(text)})`), `${name}: the message is pushed`)
}

before(async () => {
  if (skip) return
  app = await launch({ MNEMA_DESKTOP_DOWNLOAD_DIR: downloads })
})

after(async () => {
  await app?.stop()
  rmSync(downloads, { recursive: true, force: true })
})

test('the server is added by its typed address and the bundle runs as an instance', { skip }, async () => {
  const selector = await app.role('selector')
  const opened = await selector.evaluate<string>(`window.__TAURI_INTERNALS__.invoke('desktop_open_instance', { address: ${JSON.stringify(address)} }).then(() => 'ok', (e) => String(e))`)
  assert.equal(opened, 'ok')
  instance = await app.role('instance')
  await instance.watch()
  assert.equal(await instance.evaluate('location.origin'), 'mnema://localhost')
  const health = await api<{ status: string }>('GET', '/api/health')
  assert.equal(health.status, 200)
  assert.equal(health.json.status, 'ok')
})

test('sign-in works and the session lists channels', { skip }, async () => {
  assert.equal((await api('GET', '/api/channels')).status, 401)
  assert.equal((await api('POST', '/api/auth/login', { username: user, password })).status, 200)
  const channels = await api<{ categories: { channels: { id: string, type: string }[] }[] }>('GET', '/api/channels')
  assert.equal(channels.status, 200)
  channel = channels.json.categories.flatMap((c) => c.channels).find((c) => c.type === 'text')?.id ?? ''
  assert.equal(channel.length, 36)
})

test('the WebSocket opens and a sent message is pushed to it', { skip }, async () => {
  await chat('first')
})

test('the server restarts: the socket ends, requests fail meanwhile, the session survives and a new socket works', { skip }, async () => {
  appendFileSync(record, 'restart\n')
  await waitFor(() => controlHas('stopped'), 'the server stops', 120_000)
  await waitFor(() => sawEvent("e.kind === 'close'"), 'the socket closes')
  const down = await instance.evaluate<string>("fetch('/api/health').then((r) => r.ok ? 'reached' : 'down', () => 'down')")
  assert.equal(down, 'down')
  appendFileSync(record, 'down-checked\n')
  await waitFor(() => controlHas('started'), 'the server starts again', 180_000)
  await waitFor(async () => (await api('GET', '/api/channels').catch(() => ({ status: 0 }))).status === 200, 'the session works again')
  await chat('second')
})

/** A real mouse click on the middle of the element, as a person would make it. */
async function clickOn(id: string): Promise<void> {
  const box = await instance.evaluate<{ x: number, y: number }>(`(() => { const r = document.getElementById(${JSON.stringify(id)}).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`)
  for (const type of ['mousePressed', 'mouseReleased']) await instance.send('Input.dispatchMouseEvent', { type, x: box.x, y: box.y, button: 'left', clickCount: 1 })
}

test('the GDPR export is requested, saved by the host from a download link, and then reads as downloaded', { skip }, async () => {
  const requested = await api('POST', '/api/users/me/export', { current_password: password })
  assert.ok(requested.status >= 200 && requested.status < 300, `status ${requested.status}`)
  const state = async () => (await api<{ state?: string }>('GET', '/api/users/me/export')).json?.state
  await waitFor(async () => (await state()) === 'ready', 'the export is ready', 120_000)
  // The link the web app renders: an anchor with `download` to the API, in the app's own origin.
  await instance.evaluate(`(() => {
    const a = document.createElement('a')
    a.id = 'export-link'
    a.href = '/api/users/me/export/download'
    a.setAttribute('download', 'mnema-talk-export.zip')
    a.textContent = 'Download my data'
    a.style.cssText = 'position:fixed;left:8px;top:8px;z-index:2147483647;padding:12px;background:#fff;color:#000'
    document.body.append(a)
  })()`)
  await clickOn('export-link')
  await waitFor(() => readdirSync(downloads).some((name) => !name.startsWith('.')), 'the file is saved', 120_000)
  const [saved] = readdirSync(downloads).filter((name) => !name.startsWith('.'))
  // The name the web app gives the link is the one proposed (the dialog comes before the server's own).
  assert.equal(saved, 'mnema-talk-export.zip')
  const bytes = readFileSync(join(downloads, saved as string))
  assert.equal(bytes.subarray(0, 2).toString('latin1'), 'PK', 'a ZIP archive')
  assert.deepEqual(readdirSync(downloads).filter((name) => name.startsWith('.')), [], 'no temporary file is left')
  // The server records the download and keeps the file for the hour after it.
  const recorded = async () => {
    const json = (await api<{ state?: string, downloaded_at?: string }>('GET', '/api/users/me/export')).json
    return json?.state === 'ready' && typeof json.downloaded_at === 'string'
  }
  await waitFor(recorded, 'the server records the download')
})

test('earlier messages are still stored and sign-out works', { skip }, async () => {
  const history = await api('GET', `/api/channels/${channel}/messages`)
  assert.ok(JSON.stringify(history.json).includes(sent[0] ?? 'missing'))
  const out = await api('POST', '/api/auth/logout')
  assert.ok(out.status === 200 || out.status === 204)
})
