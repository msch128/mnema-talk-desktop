/**
 * End to end through the real web app: the signed bundle runs in the instance
 * window against a small fake instance, a message that mentions the signed-in
 * user arrives over the socket, and the web app itself (platform.notify ->
 * desktop_notify) must raise the toast; using the toast must make the app jump
 * to that message. Same environment as notify.e2e.ts (scripts/notify-e2e.sh).
 */
import assert from 'node:assert/strict'
import { type ChildProcess, execFile, spawn } from 'node:child_process'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { after, before, test } from 'node:test'
import { promisify } from 'node:util'
import { WebSocketServer } from 'ws'
import type { Page } from './cdp.ts'
import { type Running, launch } from './launch.ts'

const enabled = process.env.MNEMA_E2E_NOTIFY === '1'
const ME = '00000000-0000-4000-8000-0000000000a1'
const OTHER = '00000000-0000-4000-8000-0000000000a2'
const CHANNEL = '00000000-0000-4000-8000-0000000000c1'
const BACKGROUND = '00000000-0000-4000-8000-0000000000c2'
const MESSAGE = '00000000-0000-4000-8000-0000000000b1'
const NOW = '2026-10-21T12:00:00Z'

const user = { id: ME, username: 'tester', display_name: 'Tester', role: 'user', locale: 'en', bio: '', status_text: '', created_at: NOW }
const channel = { id: CHANNEL, name: 'general', number: 1, type: 'text', topic: '', category_id: null, created_at: NOW, slow_mode_seconds: 0, sort_order: 0, user_limit: 0 }
const second = { ...channel, id: BACKGROUND, name: 'random', number: 2, sort_order: 1 }
// The message is for the channel the person is not looking at: the app opens the first one.
const message = {
  id: MESSAGE, channel_id: BACKGROUND, user_id: OTHER, username: 'anna', display_name: 'Anna', content: 'hello @tester, a message for the end-to-end test', created_at: NOW, updated_at: NOW,
  attachments: [], mentions: [ME], reactions: [], reply_count: 0, is_edited: false, is_pinned: false, number: 1,
}

let server: Server
let wss: WebSocketServer
let app: Running
let instance: Page
let monitor: ChildProcess
let bus = ''
const paths: string[] = []

/** A page is found as soon as its target exists; the host's bridge and identity arrive with the document. */
async function ready(page: Page): Promise<void> {
  const deadline = Date.now() + 20_000
  while (!(await page.evaluate<boolean>("typeof window.__TAURI_INTERNALS__ === 'object' && typeof window.__MNEMA_TRUSTED_APP__ === 'object'").catch(() => false))) {
    if (Date.now() > deadline) throw new Error('the page never got the host bridge')
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
}

before(async () => {
  if (!enabled) return
  server = createServer((req, res) => {
    const url = req.url ?? ''
    paths.push(url)
    const json = (status: number, body: unknown) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)) }
    if (url === '/api/health') return json(200, { status: 'ok', version: '0.8.2', web_client_api: 1 })
    if (url === '/api/auth/me') return json(200, user)
    if (url.startsWith('/api/channels') && !url.includes('/messages')) return json(200, { categories: [], uncategorized: [channel, second] })
    if (url.includes('/messages')) return json(200, [])
    return json(404, { error: { code: 'NOT_FOUND', message: 'no' } })
  })
  wss = new WebSocketServer({ noServer: true })
  server.on('upgrade', (req, socket, head) => {
    if (req.url !== '/api/ws') { socket.destroy(); return }
    wss.handleUpgrade(req, socket, head, (ws) => {
      // The message arrives after the page had time to set itself up.
      setTimeout(() => ws.send(JSON.stringify({ type: 'message_create', payload: message })), 4000)
    })
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = `http://127.0.0.1:${(server.address() as AddressInfo).port}`
  monitor = spawn('dbus-monitor', ['--session', "interface='org.freedesktop.Notifications'"], { stdio: ['ignore', 'pipe', 'ignore'] })
  monitor.stdout?.on('data', (chunk: Buffer) => { bus += chunk.toString() })
  app = await launch()
  const selector = await app.page((t) => t.url.startsWith('mnema://localhost'))
  await ready(selector)
  await selector.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_open_instance', { address: '${address}' })`)
  instance = await app.page((t) => t.id !== selector.target.id && t.url.startsWith('mnema://localhost'))
  await ready(instance)
})

after(async () => {
  monitor?.kill()
  await app?.stop()
  wss?.close()
  server?.closeAllConnections()
  server?.close()
})

async function until(check: () => boolean | Promise<boolean>, what: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}\n--- requests ---\n${paths.join('\n')}\n--- bus ---\n${bus}\n--- page ---\n${instance.problems.join('\n')}\n--- app ---\n${app.output.join('').slice(-3000)}`)
    await new Promise((resolve) => setTimeout(resolve, 250))
  }
}

test('a mention that arrives in the web app becomes a toast, and using it opens that message', { skip: !enabled }, async () => {
  await until(() => bus.includes('member=Notify') && bus.includes('a message for the end-to-end test'), 'the web app to raise the toast')
  assert.match(bus, /Anna/)
  const before = paths.length
  const run = promisify(execFile)
  await until(async () => {
    await run('dunstctl', ['action']).catch(() => undefined)
    return paths.slice(before).some((p) => p.includes(BACKGROUND) && p.includes('/messages'))
  }, 'the app to jump to the message of the toast', 20_000)
})
