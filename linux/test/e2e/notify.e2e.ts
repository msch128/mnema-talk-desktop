/**
 * End to end, in a real Electron, against a real notification service: a page
 * of the instance window asks for a toast and the freedesktop notification
 * service receives it (seen on the session bus), and using the toast reaches
 * the page again as the click of that message.
 *
 * Needs a session bus with a notification daemon that can invoke the default
 * action of the toast on top (dunst) and dbus-monitor; `scripts/notify-e2e.sh`
 * provides both and sets MNEMA_E2E_NOTIFY, without which this file does nothing.
 */
import assert from 'node:assert/strict'
import { type ChildProcess, execFile, spawn } from 'node:child_process'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { after, before, test } from 'node:test'
import { promisify } from 'node:util'
import type { Page } from './cdp.ts'
import { type Running, launch } from './launch.ts'

const enabled = process.env.MNEMA_E2E_NOTIFY === '1'
const CHANNEL = '0b1e2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d'
const MESSAGE = '11111111-2222-4333-8444-555555555555'

let server: Server
let app: Running
let instance: Page
let monitor: ChildProcess
let bus = ''

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
    res.writeHead(req.url === '/api/health' ? 200 : 401, { 'content-type': 'application/json' })
    res.end(JSON.stringify(req.url === '/api/health' ? { status: 'ok', version: '0.8.2', web_client_api: 1 } : { error: { code: 'UNAUTHORIZED', message: 'no' } }))
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = `http://127.0.0.1:${(server.address() as AddressInfo).port}`
  // What the notification service sees: the calls to Notify and the signals back.
  monitor = spawn('dbus-monitor', ['--session', "interface='org.freedesktop.Notifications'"], { stdio: ['ignore', 'pipe', 'ignore'] })
  monitor.stdout?.on('data', (chunk: Buffer) => { bus += chunk.toString() })
  app = await launch()
  const selector = await app.role('selector')
  await ready(selector)
  await selector.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_open_instance', { address: '${address}' })`)
  instance = await app.role('instance')
  await ready(instance)
})

after(async () => {
  monitor?.kill()
  await app?.stop()
  server?.closeAllConnections()
  server?.close()
})

async function until(check: () => boolean | Promise<boolean>, what: string, timeoutMs = 20_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await check())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}\n--- bus ---\n${bus}\n--- app ---\n${app.output.join('')}`)
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
}

test('the instance window announces the capability and the service is available', { skip: !enabled }, async () => {
  const identity = await instance.evaluate<{ role: string, capabilities: string[] }>('JSON.parse(JSON.stringify(window.__MNEMA_TRUSTED_APP__))')
  assert.deepEqual([identity.role, identity.capabilities], ['instance', ['notify', 'unread', 'ws_query']])
  const call = (command: string) => instance.evaluate<string>(`window.__TAURI_INTERNALS__.invoke('${command}').then((v) => String(v), (e) => 'error ' + e)`)
  assert.equal(await call('desktop_notify_permission'), 'granted')
  assert.equal(await call('desktop_notify_request_permission'), 'granted')
})

test('a toast reaches the notification service and using it reports its message to the page', { skip: !enabled }, async () => {
  await instance.evaluate(`(async () => {
    window.__clicks = []
    const channel = window.__TAURI_INTERNALS__.transformCallback((m) => window.__clicks.push(m))
    await window.__TAURI_INTERNALS__.invoke('desktop_notify_open', { onEvent: { id: channel } })
    await window.__TAURI_INTERNALS__.invoke('desktop_notify', { title: 'Mnema end-to-end', body: 'A message for the test', channelId: '${CHANNEL}', messageId: '${MESSAGE}' })
  })()`)
  await until(() => /member=Notify\b/.test(bus) && bus.includes('Mnema end-to-end') && bus.includes('A message for the test'), 'the Notify call')
  // The default action ("default") is what a click on the toast invokes.
  assert.match(bus, /string "default"/)
  // Nothing is reported while the toast is unused.
  assert.deepEqual(await instance.evaluate('window.__clicks'), [])
  const run = promisify(execFile)
  await until(async () => {
    await run('dunstctl', ['action']).catch(() => undefined)
    return /member=ActionInvoked/.test(bus)
  }, 'the toast to be used')
  await until(async () => (await instance.evaluate<unknown[]>('window.__clicks')).length > 0, 'the click in the page')
  const clicks = await instance.evaluate<{ index: number, message: { channel_id: string, message_id: string } }[]>('window.__clicks')
  assert.deepEqual(clicks, [{ index: 0, message: { channel_id: CHANNEL, message_id: MESSAGE } }])
})

test('the badge call is accepted', { skip: !enabled }, async () => {
  assert.equal(await instance.evaluate("window.__TAURI_INTERNALS__.invoke('desktop_set_badge', { count: 3 }).then(() => 'ok', (e) => String(e))"), 'ok')
  assert.equal(await instance.evaluate("window.__TAURI_INTERNALS__.invoke('desktop_set_badge', { count: 0 }).then(() => 'ok', (e) => String(e))"), 'ok')
})

test('closing the instance takes its toasts down, so they cannot be used for another instance', { skip: !enabled }, async () => {
  const closes = () => (bus.match(/member=CloseNotification/g) ?? []).length
  const before = closes()
  await new Promise((resolve) => setTimeout(resolve, 500)) // past the host's rate limit
  await instance.evaluate(`window.__TAURI_INTERNALS__.invoke('desktop_notify', { title: 'Stale', body: 'Left over', channelId: '${CHANNEL}', messageId: '${MESSAGE}' })`)
  await until(() => bus.includes('Left over'), 'the second toast')
  await instance.send('Page.close')
  await until(() => closes() > before, 'the toast to be closed with its instance')
})
