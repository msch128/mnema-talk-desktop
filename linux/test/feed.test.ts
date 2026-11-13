/**
 * The host's feed of a server without a loaded page: it reads the counts as
 * the web app does, reads them again when the socket says something changed,
 * says the member is idle and nothing else, and stops for good.
 */
import assert from 'node:assert/strict'
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { afterEach, test } from 'node:test'
import { WebSocketServer, type WebSocket } from 'ws'
import { Jar } from '../src/core/cookies.ts'
import { Feed, IDLE, changesCounts, mentionRise, mentionText } from '../src/core/feed.ts'
import { Limits } from '../src/core/proxy.ts'
import { Turns } from '../src/core/tickets.ts'
import { flag } from '../src/core/ipcargs.ts'
import { Registry, storedFrom, storedText } from '../src/core/instances.ts'

const open: { server: Server, wss: WebSocketServer }[] = []
afterEach(async () => {
  for (const { server, wss } of open.splice(0)) {
    wss.close()
    await new Promise<void>((resolve) => { server.closeAllConnections(); server.close(() => resolve()) })
  }
})

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function fakeServer(states: () => unknown) {
  const sockets: WebSocket[] = []
  const said: string[] = []
  let reads = 0
  const server = createServer((req, res) => {
    if (req.url === '/api/read-state') {
      reads++
      const body = JSON.stringify(states())
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': String(Buffer.byteLength(body)) })
      res.end(body)
    } else { res.writeHead(404); res.end() }
  })
  const wss = new WebSocketServer({ server })
  open.push({ server, wss })
  wss.on('connection', (socket) => { sockets.push(socket); socket.on('message', (m) => said.push(String(m))) })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const origin = new URL(`http://127.0.0.1:${(server.address() as AddressInfo).port}`)
  const jar = new Jar(null, null, 0)
  jar.store(['session=s3cret; Max-Age=3600'], false, Math.floor(Date.now() / 1000))
  return { instance: { origin, jar, limits: new Limits(), now: () => Math.floor(Date.now() / 1000) }, sockets, said, reads: () => reads }
}

test('the feed reads the counts, again after a change, and its socket says only that the member is idle', async () => {
  let mentions = 1
  const server = await fakeServer(() => [{ unread_count: 3, mention_count: mentions, notify_level: 'all' }, { unread_count: 9, mention_count: 0, notify_level: 'mute' }])
  const reports: { unread: number, mentions: number }[] = []
  const feed = new Feed(server.instance, (c) => reports.push(c), { settleMs: 30, everyMs: 60_000, reopenMs: 60_000 })
  feed.start()
  for (let i = 0; i < 100 && (reports.length < 1 || server.sockets.length < 1 || server.said.length < 1); i++) await sleep(20)
  assert.deepEqual(reports[0], { unread: 1, mentions: 1 })
  assert.deepEqual(server.said, [IDLE])
  mentions = 4
  server.sockets[0]!.send(JSON.stringify({ type: 'presence_update', payload: {} }))
  server.sockets[0]!.send(JSON.stringify({ type: 'message_create', payload: {} }))
  server.sockets[0]!.send(JSON.stringify({ type: 'message_create', payload: {} }))
  for (let i = 0; i < 100 && reports.length < 2; i++) await sleep(20)
  assert.deepEqual(reports.at(-1), { unread: 1, mentions: 4 })
  assert.equal(server.reads(), 2, 'two changes in a row are one read')
  feed.stop()
  server.sockets[0]?.send(JSON.stringify({ type: 'message_create', payload: {} }))
  await sleep(100)
  assert.equal(reports.length, 2, 'a stopped feed reports nothing')
  assert.deepEqual(server.said, [IDLE], 'the host never sends typing, reads or anything else')
})

test('only what can change the counts wakes the feed', () => {
  for (const kind of ['message_create', 'message_delete', 'read_state', 'channels_changed']) assert.equal(changesCounts(JSON.stringify({ type: kind, payload: {} })), true, kind)
  for (const kind of ['presence_update', 'typing', 'voice_state_update', 'pong']) assert.equal(changesCounts(JSON.stringify({ type: kind })), false, kind)
  assert.equal(changesCounts('not json'), false)
  assert.equal(JSON.parse(IDLE).payload.idle, true)
})

test('a few servers connect at once and the rest wait their turn', async () => {
  const turns = new Turns(2)
  let running = 0
  let most = 0
  await Promise.all(Array.from({ length: 7 }, () => turns.run(async () => { running++; most = Math.max(most, running); await sleep(10); running-- })))
  assert.equal(most, 2)
})

test('the page reports busy as three true-or-false values', () => {
  assert.equal(flag({ draft: true }, 'draft'), true)
  for (const bad of [{}, { draft: 'yes' }, { draft: 1 }]) assert.throws(() => flag(bad, 'draft'), /invalid arguments/)
})

test('when hidden pages close is kept in the list file but never exported, and nonsense reads as the defaults', () => {
  const stored = storedFrom({ version: 1, servers: ['https://a.example/'], active: null, sleep: { after_secs: 1800, keep: 2 } })
  assert.ok(stored)
  const registry = Registry.restore(stored)
  assert.deepEqual(registry.sleep, { afterSecs: 1800, keep: 2 })
  assert.deepEqual(JSON.parse(storedText(registry.stored())).sleep, { after_secs: 1800, keep: 2 })
  assert.equal('sleep' in JSON.parse(storedText(registry.exported())), false)
  const junk = storedFrom({ version: 1, servers: ['https://a.example/'], active: null, sleep: 'junk' })
  assert.ok(junk)
  assert.deepEqual(Registry.restore(junk).sleep, { afterSecs: 600, keep: 4 })
})

test('a rise of the mentions is one quiet toast, a muted server gets none', () => {
  assert.equal(mentionRise(1, 4, false), 3)
  assert.equal(mentionRise(4, 4, false), null)
  assert.equal(mentionRise(4, 2, false), null)
  assert.equal(mentionRise(0, 9, true), null)
  assert.equal(mentionText(1, 'chat.example'), '1 new mention on chat.example')
  assert.equal(mentionText(3, 'chat.example'), '3 new mentions on chat.example')
})
