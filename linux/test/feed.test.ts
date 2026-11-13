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
import { Feed, IDLE, type Read, applyRead, backoffMs, changesCounts, forget, mentionRise, mentionText } from '../src/core/feed.ts'
import { Limits } from '../src/core/proxy.ts'
import { Turns } from '../src/core/tickets.ts'
import { flag } from '../src/core/ipcargs.ts'
import { Registry, type Session, storedFrom, storedText } from '../src/core/instances.ts'
import { parseEdit } from '../src/core/railui.ts'

const open: { server: Server, wss: WebSocketServer }[] = []
// Feeds a test started: stopped after it, also when it failed, so no timer keeps the run alive.
const started: Feed[] = []
afterEach(async () => {
  for (const feed of started.splice(0)) feed.stop()
  for (const { server, wss } of open.splice(0)) {
    wss.close()
    await new Promise<void>((resolve) => { server.closeAllConnections(); server.close(() => resolve()) })
  }
})

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function fakeServer(states: () => unknown, dms: () => unknown = () => [], status: () => number = () => 200) {
  const sockets: WebSocket[] = []
  const paths: string[] = []
  const said: string[] = []
  let reads = 0
  const server = createServer((req, res) => {
    const answer = req.url === '/api/read-state' ? states : req.url === '/api/dms' ? dms : null
    if (answer === null) { res.writeHead(404); res.end(); return }
    if (req.url === '/api/read-state') reads++
    const body = JSON.stringify(answer())
    res.writeHead(status(), { 'content-type': 'application/json', 'content-length': String(Buffer.byteLength(body)) })
    res.end(body)
  })
  const wss = new WebSocketServer({ server })
  open.push({ server, wss })
  wss.on('connection', (socket, req) => { sockets.push(socket); paths.push(req.url ?? ''); socket.on('message', (m) => said.push(String(m))) })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const origin = new URL(`http://127.0.0.1:${(server.address() as AddressInfo).port}`)
  const jar = new Jar(null, null, 0)
  jar.store(['session=s3cret; Max-Age=3600'], false, Math.floor(Date.now() / 1000))
  return { instance: { origin, jar, limits: new Limits(), now: () => Math.floor(Date.now() / 1000) }, sockets, paths, said, reads: () => reads }
}

const QUICK = { settleMs: 30, everyMs: 60_000, reopenMs: 60_000, reopenMaxMs: 60_000 }

test('the feed reads the counts with the conversations, again after a change, and its socket says only that the member is idle', async () => {
  let mentions = 1
  let dmMentions = 0
  const server = await fakeServer(() => [{ unread_count: 3, mention_count: mentions, notify_level: 'all' }, { unread_count: 9, mention_count: 0, notify_level: 'mute' }], () => [{ id: 'd1', unread_count: 1, mention_count: dmMentions, notify_level: 'inherit' }])
  const reports: Read[] = []
  const feed = new Feed(server.instance, (r) => reports.push(r), QUICK)
  started.push(feed)
  feed.start()
  for (let i = 0; i < 100 && (reports.length < 1 || server.sockets.length < 1 || server.said.length < 1); i++) await sleep(20)
  // The muted channel adds no unread; the conversation adds one.
  assert.deepEqual(reports[0], { kind: 'counts', unread: 2, mentions: 1 })
  assert.deepEqual(server.said, [IDLE])
  // With the conversation query: the hub sends conversation events only then.
  assert.deepEqual(server.paths, ['/api/ws?dm=1'])
  mentions = 4
  server.sockets[0]!.send(JSON.stringify({ type: 'presence_update', payload: {} }))
  server.sockets[0]!.send(JSON.stringify({ type: 'message_create', payload: {} }))
  server.sockets[0]!.send(JSON.stringify({ type: 'message_create', payload: {} }))
  for (let i = 0; i < 100 && reports.length < 2; i++) await sleep(20)
  assert.deepEqual(reports.at(-1), { kind: 'counts', unread: 2, mentions: 4 })
  assert.equal(server.reads(), 2, 'two changes in a row are one read')
  // A mention in a direct message counts too, after its event.
  dmMentions = 2
  server.sockets[0]!.send(JSON.stringify({ type: 'dm_prefs_updated', payload: {} }))
  for (let i = 0; i < 100 && reports.length < 3; i++) await sleep(20)
  assert.deepEqual(reports.at(-1), { kind: 'counts', unread: 2, mentions: 6 })
  feed.stop()
  server.sockets[0]?.send(JSON.stringify({ type: 'message_create', payload: {} }))
  await sleep(100)
  assert.equal(reports.length, 3, 'a stopped feed reports nothing')
  assert.deepEqual(server.said, [IDLE], 'the host never sends typing, reads or anything else')
})

test('a signed-out session ends the feed once, and is never sent a socket', async () => {
  const server = await fakeServer(() => [], () => [], () => 401)
  const reports: Read[] = []
  const feed = new Feed(server.instance, (r) => reports.push(r), QUICK)
  started.push(feed)
  feed.start()
  for (let i = 0; i < 100 && reports.length < 1; i++) await sleep(20)
  await sleep(100)
  assert.deepEqual(reports, [{ kind: 'signed_out' }])
  assert.equal(feed.running, false)
  assert.equal(server.sockets.length, 0)
})

test('a server that is down is never sent a socket', async () => {
  const server = await fakeServer(() => [], () => [], () => 502)
  const reports: Read[] = []
  const feed = new Feed(server.instance, (r) => reports.push(r), QUICK)
  started.push(feed)
  feed.start()
  for (let i = 0; i < 100 && reports.length < 1; i++) await sleep(20)
  await sleep(100)
  feed.stop()
  assert.deepEqual(reports, [{ kind: 'unreachable' }])
  assert.equal(server.sockets.length, 0)
})

test('an unreachable server is asked later and later, with jitter, up to a bound', () => {
  const times = { settleMs: 2000, everyMs: 60_000, reopenMs: 30_000, reopenMaxMs: 300_000 }
  assert.deepEqual([1, 2, 3, 5, 40].map((n) => backoffMs(times, n, 0)), [30_000, 60_000, 120_000, 300_000, 300_000])
  const jittered = backoffMs(times, 1, 0.99)
  assert.ok(jittered >= 24_000 && jittered <= 30_000, String(jittered))
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
  const stored = storedFrom({ version: 1, servers: ['https://a.example/'], active: null, sleep: { after_secs: 3600, keep: 2 } })
  assert.ok(stored)
  const registry = Registry.restore(stored)
  assert.deepEqual(registry.sleep, { afterSecs: 3600, keep: 2 })
  assert.deepEqual(JSON.parse(storedText(registry.stored())).sleep, { after_secs: 3600, keep: 2 })
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

test('unload idle servers offers off, ten minutes and an hour, keeps the number of pages, and is kept with the list', () => {
  const stored = storedFrom({ version: 1, servers: ['https://a.example/'], active: null })
  assert.ok(stored)
  const registry = Registry.restore(stored)
  assert.equal(registry.rail(true).unload_after, 600)
  assert.equal(registry.setUnloadAfter(3600), true)
  assert.equal(registry.rail(true).unload_after, 3600)
  assert.equal(registry.setUnloadAfter(null), true)
  assert.equal(registry.rail(true).unload_after, null)
  for (const other of [0, 60, 601, 86_400]) assert.equal(registry.setUnloadAfter(other), false, String(other))
  assert.equal(registry.sleep.keep, 4)
  const again = storedFrom(JSON.parse(storedText(registry.stored())))
  assert.ok(again)
  assert.equal(Registry.restore(again).sleep.afterSecs, null)
  assert.deepEqual(parseEdit({ op: 'unload_after', until: -1 }), { op: 'unload_after', until: -1 })
  assert.deepEqual(parseEdit({ op: 'unload_after', until: 3600 }), { op: 'unload_after', until: 3600 })
  for (const bad of [0, 1800, '600', null]) assert.throws(() => parseEdit({ op: 'unload_after', until: bad }), /invalid arguments/, String(bad))
})

test('the feed asks a server that is down later each time', async () => {
  const server = await fakeServer(() => [], () => [], () => 502)
  const at: number[] = []
  const feed = new Feed(server.instance, () => at.push(performance.now()), { settleMs: 30, everyMs: 60_000, reopenMs: 200, reopenMaxMs: 10_000 })
  started.push(feed)
  feed.start()
  for (let i = 0; i < 100 && at.length < 3; i++) await sleep(20)
  feed.stop()
  assert.ok(at.length >= 3, `${at.length} reads`)
  const [first, second] = [at[1]! - at[0]!, at[2]! - at[1]!]
  // 200 ms, then 400 ms, each up to a fifth earlier.
  assert.ok(first >= 155, String(first))
  assert.ok(second >= 315 && second > first, `${first} then ${second}`)
})

test('a read puts the counts on the slot, and a signed-out session clears them', () => {
  const registry = new Registry()
  const id = registry.add(new URL('https://feed.example/'))
  const slot = registry.slot(id)!
  slot.phase = { kind: 'ready', session: {} as Session }
  const name = registry.displayName(slot)
  // The first numbers after the start are what was there already: no toast. A rise after that is news.
  assert.deepEqual(applyRead(registry, id, { kind: 'counts', unread: 3, mentions: 2 }), { kind: 'shown', rise: null, name })
  assert.deepEqual(applyRead(registry, id, { kind: 'counts', unread: 3, mentions: 5 }), { kind: 'shown', rise: 3, name })
  assert.deepEqual(applyRead(registry, id, { kind: 'unreachable' }), { kind: 'kept' })
  assert.deepEqual([slot.unread, slot.mentions], [3, 5], 'no answer keeps them')
  assert.deepEqual(applyRead(registry, id, { kind: 'signed_out' }), { kind: 'shown', rise: null, name })
  assert.deepEqual([slot.unread, slot.mentions], [0, 0])
  assert.deepEqual(applyRead(registry, 'gone', { kind: 'counts', unread: 1, mentions: 1 }), { kind: 'gone' })
  // After a restart of the server the first numbers are a new start.
  registry.restart(id)
  slot.phase = { kind: 'ready', session: {} as Session }
  assert.deepEqual(applyRead(registry, id, { kind: 'counts', unread: 1, mentions: 7 }), { kind: 'shown', rise: null, name })
  assert.deepEqual(applyRead(registry, id, { kind: 'counts', unread: 1, mentions: 8 }), { kind: 'shown', rise: 1, name })
})

test('an ending feed forgets only itself', () => {
  const instance = {} as ConstructorParameters<typeof Feed>[0]
  const [old, newer] = [new Feed(instance, () => {}), new Feed(instance, () => {})]
  // Its page took over, failed to open, and a new feed started.
  const feeds = new Map([['a', newer]])
  forget(feeds, 'a', old)
  assert.equal(feeds.get('a'), newer, 'the newer feed stays')
  forget(feeds, 'a', newer)
  assert.equal(feeds.has('a'), false)
})
