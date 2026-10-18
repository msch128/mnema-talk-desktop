import assert from 'node:assert/strict'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { readFileSync } from 'node:fs'
import { appApi, bundled, localOrigin, pickerUi, updateUi } from '../src/core/origin.ts'
import { Trusted, type TrustedDeps, defaultDeps, pinnedKeys } from '../src/core/trusted.ts'
import { parseUtc } from '../src/core/trust.ts'
import { ROOT_PUB } from '../src/core/pinned.ts'
import { bundle, spec, testKey, trusted as keysFor } from './helpers/fixtures.ts'
import { reply, serve, type TestServer } from './helpers/server.ts'

const NOW = 1_793_491_200 // 2026-11-01T00:00:00Z
const fixture = (name: string) => readFileSync(new URL(`../../experimental/app/fixtures/${name}`, import.meta.url))
const doc = fixture('trust.json')
const sig = fixture('trust.json.sig')

const dirs: string[] = []
const servers: TestServer[] = []
afterEach(async () => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
  for (const s of servers.splice(0)) await s.close()
})
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-trusted-')); dirs.push(d); return d }

function deps(change: Partial<TrustedDeps> = {}): TrustedDeps {
  return { ...defaultDeps(null), now: () => NOW, log: () => {}, ...change }
}

const url = (raw: string) => new URL(raw)

test('only the hosts own origin and its routes are app pages', () => {
  const id = '11111111-1111-1111-1111-111111111111'
  const app = (path: string) => new URL(path, 'mnema://localhost/')
  for (const path of ['', '/', '/index.html', '/login', '/admin/users', `/c/${id}`, `/v/${id}/chat`]) assert.ok(bundled(app(path)), path)
  for (const path of ['/other.html', '/api/health', '/admin/unknown', '/c/not-an-id']) assert.ok(!bundled(app(path)), path)
  for (const raw of ['https://community.example/', 'tauri://localhost/', 'http://tauri.localhost/', 'mnema://localhost:9000/', 'mnema://user@localhost/', 'mnema://evil/', 'https://mnema.localhost.evil.example/', 'https://mnema.localhost/']) {
    assert.ok(!bundled(url(raw)), raw)
  }
})

test('api paths on the app origin are the proxys', () => {
  const app = (path: string) => new URL(path, 'mnema://localhost/')
  assert.ok(appApi(app('/api/media/1')))
  assert.ok(!appApi(app('/c/1')))
  assert.ok(!appApi(app('/apix')))
  assert.ok(!appApi(url('https://community.example/api/media/1')))
  assert.ok(updateUi(app('/__update/index.html')))
  assert.ok(!updateUi(app('/__updatex')))
  assert.ok(pickerUi(app('/__picker/index.html')))
  assert.ok(!pickerUi(url('https://community.example/__picker/')))
})

test('local origins compare as text because custom origins are opaque', () => {
  assert.equal(localOrigin(url('mnema://localhost/')), localOrigin(url('mnema://localhost/c/1')))
  assert.equal(localOrigin(url('https://community.example/')), null)
})

test('the pinned bundle key and root key are in effect by default', () => {
  assert.equal(pinnedKeys().length, 1)
  assert.equal(ROOT_PUB.length, 64)
})

test('only a statement from the pinned root key changes the keys in force', () => {
  const t = new Trusted(null, null, deps())
  const before = t.keysInForce().map((k) => k.id)
  const zero = Buffer.from(Buffer.alloc(64).toString('base64'))
  assert.throws(() => t.applyTrust(doc, zero, NOW))
  const tampered = Buffer.from(doc)
  tampered[12] ^= 1
  assert.throws(() => t.applyTrust(tampered, sig, NOW))
  // The real statement lists the pinned key: nothing is dropped.
  assert.equal(t.applyTrust(doc, sig, NOW), false)
  assert.deepEqual(t.keysInForce().map((k) => k.id), before)
  // Applying it again is a no-op.
  assert.equal(t.applyTrust(doc, sig, NOW), false)
})

test('installs need a fresh statement and a successful fetch in this round', () => {
  const expiry = parseUtc('2027-04-15T22:00:57Z') as number
  const t = new Trusted(null, null, deps())
  assert.equal(t.fresh(), false, 'no statement yet: no installs')
  // Offline: still nothing fresh, nothing dropped.
  assert.equal(t.applyRound(null, NOW), false)
  assert.equal(t.fresh(), false)
  // The statement arrives.
  t.applyRound({ doc, sig }, NOW)
  assert.equal(t.fresh(), true)
  const count = t.keysInForce().length
  // A later failed fetch keeps the last good statement, but installs stop.
  t.applyRound(null, NOW)
  assert.equal(t.fresh(), false)
  assert.equal(t.keysInForce().length, count)
  // The same statement is fresh again once fetched.
  t.applyRound({ doc, sig }, NOW)
  assert.equal(t.fresh(), true)
  // Past its expiry the replay is refused, installs stay off and the keys of the expired statement stay in force.
  t.applyRound({ doc, sig }, expiry + 1)
  assert.equal(t.fresh(), false)
  assert.equal(t.keysInForce().length, count)
})

test('a client without any bundle says so instead of loading server code', () => {
  const t = new Trusted(null, null, deps())
  assert.equal(t.view(), undefined)
  assert.equal(t.session(), null)
  assert.deepEqual(t.store.emptyReason(), { kind: 'no_bundle' })
  const withDir = new Trusted(tmp(), null, deps())
  assert.notEqual(withDir.jarName(url('https://a.example/')), withDir.jarName(url('https://b.example/')))
})

test('a statement that withdraws the key of the running bundle ends the session', async () => {
  // The test statement allows only key 2; the bundle is signed by key 7 and was trusted via the default keys.
  const server = await health({ status: 'ok', version: '0.9.0', web_client_api: 1 })
  const withKey7 = deps({ pinned: () => keysFor(7), root: testKey(9).hex, discover: async () => ({ version: [0, 9, 0], webClientApi: 1 }) })
  const t = new Trusted(null, bundle(spec('0.9.0')), withKey7)
  const session = await t.openInstance(server.origin.href)
  assert.deepEqual(session.held.bundle.version, [0, 9, 0])
  assert.equal(t.view(), session.held)
  assert.equal(t.session(), session)
  await assert.rejects(t.openInstance(server.origin.href), /Close the current instance/)
  const text = JSON.stringify({ schema: 1, serial: 1, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z', bundle_keys: [testKey(2).hex], revoked_bundle_keys: [testKey(7).id], updater_keys: [], revoked_updater_keys: [] })
  const { signText } = await import('./helpers/fixtures.ts')
  assert.equal(t.applyTrust(Buffer.from(text), Buffer.from(signText(testKey(9), text)), NOW), true)
  assert.equal(t.session(), null, 'the session on a withdrawn bundle is ended')
  assert.equal(t.view(), undefined)
})

async function health(body: object): Promise<TestServer> {
  const server = await serve(reply(200, { 'content-type': 'application/json' }, JSON.stringify(body)))
  servers.push(server)
  return server
}

test('opening an instance picks the verified bundle by the server health and never the servers code', async () => {
  const server = await health({ status: 'ok', version: '0.9.0', web_client_api: 1 })
  const t = new Trusted(null, bundle(spec('0.8.0')), deps({ pinned: () => keysFor(7) }))
  const session = await t.openInstance(server.origin.href)
  assert.deepEqual(session.held.bundle.version, [0, 8, 0])
  assert.equal(session.instance.origin.href, `${server.origin.href}`)
  t.closeSession()
  // A newer contract than any bundle: no update is installed without a fresh statement, and the client says so.
  const newer = await health({ status: 'ok', version: '1.0.0', web_client_api: 2 })
  const updates: string[] = []
  await assert.rejects(
    new Trusted(null, bundle(spec('0.8.0')), deps({ pinned: () => keysFor(7), installNewer: async () => { updates.push('tried') }, fetchTrust: async () => { throw new Error('offline') } })).openInstance(newer.origin.href),
    /needs a newer Mnema app/,
  )
  assert.deepEqual(updates, [], 'nothing is installed without a fresh trust statement')
  // Addresses that are not an HTTPS origin are refused before any request.
  await assert.rejects(t.openInstance('http://community.example'), /Use an HTTPS origin/)
  // An instance that is down.
  const down = await serve(reply(503, {}, 'x'))
  servers.push(down)
  await assert.rejects(t.openInstance(down.origin.href), /unavailable or redirects/)
})
