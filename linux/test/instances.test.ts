import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { Jar, type Vault } from '../src/core/cookies.ts'
import { AddError, MAX_COUNT, MAX_INSTANCES, Registry, emptyStored, idFor, loadStored, saveStored, type Session } from '../src/core/instances.ts'
import { Limits, forward } from '../src/core/proxy.ts'
import { Sockets } from '../src/core/sockets.ts'
import type { Held } from '../src/core/store.ts'

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-instances-')); dirs.push(d); return d }

const url = (host: string) => new URL(`https://${host}/`)
const id = (host: string) => idFor(url(host))
function registry(...hosts: string[]): Registry {
  const r = new Registry()
  for (const h of hosts) r.add(url(h))
  return r
}

class Memory implements Vault {
  items = new Map<string, Buffer>()
  load(account: string) { return this.items.get(account) ?? null }
  save(account: string, secret: Buffer) { this.items.set(account, secret); return true }
  remove(account: string) { this.items.delete(account) }
}

function session(r: Registry, serverId: string, vault: Vault, dir: string | null = null): Session {
  const origin = r.slot(serverId)!.origin
  const jar = new Jar(dir ? join(dir, `${serverId}.json`) : null, { vault, account: serverId }, 0)
  return { id: serverId, instance: { origin, jar, limits: new Limits(), now: () => 10 }, held: {} as Held, sockets: new Sockets() }
}

test('a server is always the same slot, named by its origin', () => {
  assert.equal(id('a.example'), id('a.example'))
  assert.notEqual(id('a.example'), id('b.example'))
  assert.notEqual(id('a.example'), idFor(new URL('https://a.example:8443/')))
  assert.match(id('a.example'), /^[0-9a-f]{16}$/)
})

test('the same server cannot be added twice and nine is the most', () => {
  const r = registry('a.example')
  assert.throws(() => r.add(url('a.example')), (e: unknown) => e instanceof AddError && e.kind === 'known' && e.id === id('a.example'))
  for (let n = 1; n < MAX_INSTANCES; n++) r.add(url(`s${n}.example`))
  assert.equal(r.length, MAX_INSTANCES)
  assert.throws(() => r.add(url('more.example')), (e: unknown) => e instanceof AddError && e.kind === 'full')
  assert.equal(r.startAdding(), false)
  assert.equal(r.rail(true).can_add, false)
})

test('switching follows the list and the number keys', () => {
  const r = registry('a.example', 'b.example', 'c.example')
  assert.ok(r.activate(id('b.example')))
  assert.equal(r.activateNth(3), id('c.example'))
  assert.equal(r.activateNth(1), id('a.example'))
  for (const n of [0, 4, 9, 1.5, -1]) assert.equal(r.activateNth(n), null, String(n))
  assert.equal(r.active(), id('a.example'))
  assert.equal(r.activate('0000000000000000'), false)
})

test('the page for adding a server comes and goes without losing the front', () => {
  const r = registry('a.example')
  r.activate(id('a.example'))
  assert.ok(r.startAdding())
  assert.equal(r.adding(), true)
  assert.equal(r.active(), id('a.example'))
  r.stopAdding()
  assert.equal(r.adding(), false)
  const empty = new Registry()
  empty.stopAdding()
  assert.ok(empty.startAdding())
  assert.equal(empty.rail(true).adding, true)
})

test('removing the front server hands the front to its neighbour', () => {
  const r = registry('a.example', 'b.example', 'c.example')
  r.activate(id('b.example'))
  assert.equal(r.remove(id('b.example'))?.id, id('b.example'))
  assert.equal(r.active(), id('c.example'))
  r.remove(id('c.example'))
  assert.equal(r.active(), id('a.example'))
  r.add(url('d.example'))
  r.remove(id('d.example'))
  assert.equal(r.active(), id('a.example'))
  assert.ok(r.remove(id('a.example')))
  assert.equal(r.active(), null)
  assert.equal(r.adding(), true)
  assert.equal(r.remove(id('a.example')), null)
})

test('reordering moves only the one server', () => {
  const r = registry('a.example', 'b.example', 'c.example')
  assert.ok(r.moveTo(id('c.example'), 0))
  assert.deepEqual(r.ids(), [id('c.example'), id('a.example'), id('b.example')])
  assert.ok(r.moveTo(id('c.example'), 99))
  assert.equal(r.ids().at(-1), id('c.example'))
  assert.equal(r.moveTo('0000000000000000', 0), false)
})

test('counts belong to the server that reported them and are bounded', () => {
  const r = registry('a.example', 'b.example')
  assert.ok(r.report(id('b.example'), 4, 2))
  const rail = r.rail(true)
  assert.deepEqual([rail.instances[0]!.unread, rail.instances[0]!.mentions], [0, 0])
  assert.deepEqual([rail.instances[1]!.unread, rail.instances[1]!.mentions], [4, 2])
  r.report(id('a.example'), 1e12, Number.NaN)
  assert.deepEqual([r.slot(id('a.example'))!.unread, r.slot(id('a.example'))!.mentions], [MAX_COUNT, 0])
  r.report(id('a.example'), 0, 1e12)
  assert.equal(r.badge(), MAX_COUNT, 'the sum is bounded too')
  assert.equal(r.report('0000000000000000', 1, 1), false)
})

test('the plain badge of an older bundle stands in until the page reports both', () => {
  const r = registry('a.example')
  r.reportBadge(id('a.example'), 3)
  assert.deepEqual([r.slot(id('a.example'))!.unread, r.slot(id('a.example'))!.mentions], [1, 3])
  r.report(id('a.example'), 5, 1)
  r.reportBadge(id('a.example'), 9)
  assert.deepEqual([r.slot(id('a.example'))!.unread, r.slot(id('a.example'))!.mentions], [5, 1])
  r.restart(id('a.example'))
  assert.equal(r.badge(), 0)
})

test('a toast belongs to the run it was shown for', () => {
  const r = registry('a.example', 'b.example')
  const generation = r.slot(id('a.example'))!.generation
  assert.equal(r.byGeneration(generation)?.id, id('a.example'))
  assert.equal(r.byGeneration(''), undefined)
  assert.equal(r.byGeneration('someone-else'), undefined)
  r.restart(id('a.example'))
  assert.equal(r.byGeneration(generation), undefined)
})

test('the rail has the shape the rail page reads', () => {
  const r = registry('www.chat.example', '10.0.0.1')
  r.add(new URL('https://talk.example:8443/'))
  r.slot(id('10.0.0.1'))!.phase = { kind: 'refused', why: 'Too old.' }
  const rail = r.rail(true)
  assert.deepEqual(rail.instances.map((i) => i.name), ['www.chat.example', '10.0.0.1', 'talk.example:8443'])
  assert.deepEqual(rail.instances.map((i) => i.initial), ['C', '1', 'T'])
  assert.deepEqual(rail.instances.map((i) => i.status), ['starting', 'refused', 'starting'])
  assert.equal(rail.instances[1]!.message, 'Too old.')
  assert.ok(rail.instances.every((i) => i.hue >= 0 && i.hue < 360))
  assert.deepEqual(Object.keys(rail).sort(), ['active', 'adding', 'can_add', 'instances'])
  assert.deepEqual(Object.keys(rail.instances[0]!).sort(), ['hue', 'id', 'initial', 'mentions', 'message', 'name', 'status', 'unread'])
  assert.equal(r.rail(false).can_add, false)
})

test('each server keeps its cookies and its stored session to itself', () => {
  const r = registry('a.example', 'b.example')
  const vault = new Memory()
  const a = session(r, id('a.example'), vault)
  const b = session(r, id('b.example'), vault)
  a.instance.jar.store(['session=from-a; Max-Age=3600; Secure'], true, 10)
  assert.equal(a.instance.jar.header(11), 'session=from-a')
  assert.equal(b.instance.jar.header(11), null, 'B never sends A\'s cookie')
  b.instance.jar.store(['session=from-b; Max-Age=3600; Secure'], true, 10)
  assert.equal(a.instance.jar.header(11), 'session=from-a')
  assert.deepEqual([...vault.items.keys()].sort(), [id('a.example'), id('b.example')].sort())
  assert.notDeepEqual(vault.items.get(id('a.example')), vault.items.get(id('b.example')))
  // A restart reads back each one's own.
  assert.equal(session(r, id('a.example'), vault).instance.jar.header(11), 'session=from-a')
})

test('removing a server wipes its session and only its session', () => {
  const dir = tmp()
  const r = registry('a.example', 'b.example')
  const vault = new Memory()
  const a = session(r, id('a.example'), vault, dir)
  const b = session(r, id('b.example'), vault, dir)
  for (const s of [a, b]) s.instance.jar.store(['session=x; Max-Age=3600; Secure'], true, 10)
  const file = join(dir, `${id('a.example')}.json`)
  writeFileSync(file, '[]')
  r.slot(id('a.example'))!.phase = { kind: 'ready', session: a }
  const gone = r.remove(id('a.example'))!
  assert.equal(gone.phase.kind, 'ready')
  a.instance.jar.forget()
  assert.equal(a.instance.jar.header(11), null)
  assert.equal(vault.load(id('a.example')), null, 'the secret store entry is gone')
  assert.equal(existsSync(file), false, 'and the fallback file')
  assert.equal(b.instance.jar.header(11), 'session=x')
  assert.ok(vault.load(id('b.example')))
  assert.equal(r.session(id('a.example')), null, 'the removed server leads nowhere')
})

test('only a running server has a session', () => {
  const r = registry('a.example')
  assert.equal(r.session(id('a.example')), null)
  r.slot(id('a.example'))!.phase = { kind: 'ready', session: session(r, id('a.example'), new Memory()) }
  assert.ok(r.session(id('a.example')))
  r.slot(id('a.example'))!.phase = { kind: 'refused', why: 'no' }
  assert.equal(r.session(id('a.example')), null)
})

test('the list survives a restart with addresses only', () => {
  const file = join(tmp(), 'instances.json')
  const r = registry('a.example', 'b.example')
  r.activate(id('b.example'))
  saveStored(file, r.stored())
  const text = readFileSync(file, 'utf8')
  assert.ok(!/cookie|session/i.test(text))
  const back = Registry.restore(loadStored(file))
  assert.deepEqual(back.ids(), r.ids())
  assert.equal(back.active(), id('b.example'))
  assert.equal(back.adding(), false)
  assert.equal(Registry.restore(emptyStored()).adding(), true)
})

test('a list that does not hold up is not trusted', () => {
  const file = join(tmp(), 'instances.json')
  for (const bad of ['not json', '{"version":2,"servers":["https://a.example/"],"active":null}', '{"servers":["https://a.example/"]}']) {
    writeFileSync(file, bad)
    assert.deepEqual(loadStored(file).servers, [], bad)
  }
  const many = Array.from({ length: 20 }, (_, n) => `https://s${n}.example/`)
  writeFileSync(file, JSON.stringify({ version: 1, servers: ['http://plain.example/', 'https://a.example/path', 'https://a.example/', 'https://a.example/', 'javascript:alert(1)', '', 7, ...many], active: 'https://unknown.example/' }))
  const stored = loadStored(file)
  assert.equal(stored.servers.length, MAX_INSTANCES)
  assert.equal(stored.servers[0]!.href, 'https://a.example/')
  assert.equal(stored.active, null, 'the front must be a server on the list')
  assert.equal(Registry.restore(stored).active(), id('a.example'))
  writeFileSync(file, ' '.repeat(16 * 1024 + 1))
  assert.deepEqual(loadStored(file).servers, [])
  assert.deepEqual(loadStored(null).servers, [])
  assert.deepEqual(loadStored(join(tmp(), 'missing.json')).servers, [])
})

test('the requests of two servers carry only their own cookies', async () => {
  const { serve } = await import('./helpers/server.ts')
  const handler = (name: string) => (_r: unknown, res: import('node:http').ServerResponse) => {
    res.writeHead(200, { 'content-type': 'application/json', 'set-cookie': `session=${name}-token; Max-Age=3600; Path=/` })
    res.end('{}')
  }
  const one = await serve(handler('a'))
  const two = await serve(handler('b'))
  try {
    const r = new Registry()
    r.add(one.origin)
    r.add(two.origin)
    const vault = new Memory()
    const a = session(r, idFor(one.origin), vault)
    const b = session(r, idFor(two.origin), vault)
    await forward(a.instance, 'GET', '/api/login', [], new Uint8Array())
    await forward(a.instance, 'GET', '/api/echo', [], new Uint8Array())
    await forward(b.instance, 'GET', '/api/echo', [], new Uint8Array())
    assert.equal(one.requests[1]!.headers.cookie, 'session=a-token')
    assert.equal(two.requests[0]!.headers.cookie, undefined, 'B was not sent A\'s cookie')
    await forward(b.instance, 'GET', '/api/echo', [], new Uint8Array())
    assert.equal(two.requests[1]!.headers.cookie, 'session=b-token')
    assert.equal(one.requests[1]!.headers.cookie, 'session=a-token')
    assert.equal(one.requests[0]!.headers.origin, one.origin.origin)
  } finally {
    await one.close()
    await two.close()
  }
})

test('a list that names a server to keep the old storage is read without it: no server ever inherits it', () => {
  const file = join(tmp(), 'instances.json')
  writeFileSync(file, JSON.stringify({ version: 1, servers: ['https://a.example/'], active: null, legacy: id('a.example') }))
  const stored = loadStored(file)
  assert.equal('legacy' in stored, false)
  const r = Registry.restore(stored)
  assert.deepEqual(r.stored(), { servers: [new URL('https://a.example/')], active: new URL('https://a.example/') })
})
