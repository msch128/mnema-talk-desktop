import assert from 'node:assert/strict'
import { mkdtempSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { Store } from '../src/core/store.ts'
import { bundleAsset, checkAndInstall, fetchLimited, fetchTrust, updatesDisabled } from '../src/core/updates.ts'
import { bundle, spec, trusted } from './helpers/fixtures.ts'
import { reply, serve, type TestServer } from './helpers/server.ts'

const servers: TestServer[] = []
const dirs: string[] = []
afterEach(async () => {
  for (const s of servers.splice(0)) await s.close()
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})
const start: typeof serve = async (h) => { const s = await serve(h); servers.push(s); return s }
const LOOPBACK = { allowHttp: true }

const release = (tag: string, assets: unknown[]) => Buffer.from(JSON.stringify({ tag_name: tag, assets }))
const asset = (name: string, url: string, size = 10) => ({ name, browser_download_url: url, size })

test('the release asset is the bundle of the tagged version, over https, without credentials', () => {
  const ok = bundleAsset(release('v0.9.1', [asset('other.zip', 'https://x.example/o.zip'), asset('mnema-web-0.9.1.zip', 'https://github.com/a/b/releases/download/v0.9.1/mnema-web-0.9.1.zip')]))
  assert.deepEqual(ok?.version, [0, 9, 1])
  assert.equal(ok?.url.href, 'https://github.com/a/b/releases/download/v0.9.1/mnema-web-0.9.1.zip')
  for (const bad of [
    release('0.9.1', [asset('mnema-web-0.9.1.zip', 'https://x.example/a')]),
    release('v0.9', [asset('mnema-web-0.9.zip', 'https://x.example/a')]),
    release('v0.9.1', [asset('mnema-web-0.9.0.zip', 'https://x.example/a')]),
    release('v0.9.1', [asset('mnema-web-0.9.1.zip', 'http://x.example/a')]),
    release('v0.9.1', [asset('mnema-web-0.9.1.zip', 'https://user:pw@x.example/a')]),
    release('v0.9.1', [asset('mnema-web-0.9.1.zip', 'https://x.example/a', 97 << 20)]),
    release('v0.9.1', [asset('mnema-web-0.9.1.zip', 'https://x.example/a'), { name: 5 }]),
    Buffer.from('nope'),
  ]) {
    assert.equal(bundleAsset(bad), null, bad.toString())
  }
})

test('the update check can be switched off', () => {
  assert.equal(updatesDisabled({ MNEMA_DESKTOP_NO_UPDATE_CHECK: '1' }), true)
  assert.equal(updatesDisabled({ MNEMA_DESKTOP_NO_UPDATE_CHECK: '0' }), false)
  assert.equal(updatesDisabled({ MNEMA_DESKTOP_NO_UPDATE_CHECK: '' }), false)
  assert.equal(updatesDisabled({}), false)
})

test('redirects are followed over https only, up to five, and the size is bounded', async () => {
  const target = await start(reply(200, {}, 'payload'))
  const hop = await start(reply(302, { location: new URL('/x', target.origin).href }, ''))
  assert.equal((await fetchLimited(new URL('/y', hop.origin), 'text/plain', 100, LOOPBACK)).toString(), 'payload')
  // A plain http redirect target is not followed in production.
  await assert.rejects(fetchLimited(new URL('/y', hop.origin), 'text/plain', 100, { allowHttp: false }), /unsupported scheme/)
  const downgrade = await start(reply(302, { location: 'http://127.0.0.1:1/x' }, ''))
  await assert.rejects(fetchLimited(new URL('/y', downgrade.origin), 'text/plain', 100, LOOPBACK).then(() => 'followed'), /unreachable|status|refused|ECONN/)
  const loop = await start((_r, res) => { res.writeHead(302, { location: '/again' }); res.end() })
  await assert.rejects(fetchLimited(new URL('/y', loop.origin), 'text/plain', 100, LOOPBACK), /too many redirects/)
  await assert.rejects(fetchLimited(new URL('/y', target.origin), 'text/plain', 3, LOOPBACK), /too large/)
  const missing = await start(reply(404, {}, 'no'))
  await assert.rejects(fetchLimited(new URL('/y', missing.origin), 'text/plain', 100, LOOPBACK), /status 404/)
})

test('the trust statement and its signature are fetched unverified and bounded', async () => {
  const server = await start((r, res) => { res.writeHead(200, { 'content-length': '3' }); res.end(r.url === '/t' ? 'doc' : 'sig') })
  const { doc, sig } = await fetchTrust({ trust: new URL('/t', server.origin), trustSig: new URL('/s', server.origin), fetch: LOOPBACK })
  assert.deepEqual([doc.toString(), sig.toString()], ['doc', 'sig'])
})

test('a newer release is downloaded, verified and installed; replayed or unsigned ones are not', async () => {
  const d = mkdtempSync(join(tmpdir(), 'mnema-updates-'))
  dirs.push(d)
  const store = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  let tag = 'v0.8.1'
  let version = spec('0.8.1')
  const server = await start((r, res) => {
    const base = `http://127.0.0.1:${r.headers.host?.split(':')[1]}`
    if (r.url === '/latest') {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(release(tag, [asset(`mnema-web-${tag.slice(1)}.zip`, `${base}/asset`)]))
      return
    }
    const body = bundle(version)
    res.writeHead(200, { 'content-length': String(body.length) })
    res.end(body)
  })
  const sources = { latest: new URL('/latest', server.origin), fetch: LOOPBACK }
  const keys = () => trusted(7)
  await assert.rejects(checkAndInstall(store, keys, { latest: sources.latest, fetch: { allowHttp: false } }), /unsupported scheme/, 'plain http is not used in production')
  assert.deepEqual(await checkAndInstall(store, keys, sources), [0, 8, 1])
  assert.equal(store.newest()?.source, 'cache')
  assert.ok(readdirSync(d).includes('mnema-web-0.8.1.zip'))
  // Nothing newer than what is held: no download.
  const before = server.requests.length
  assert.equal(await checkAndInstall(store, keys, sources), null)
  assert.equal(server.requests.length, before + 1)
  // A release whose bundle is signed by another key is not installed.
  tag = 'v0.8.2'
  version = spec('0.8.2', { seed: 9 })
  await assert.rejects(checkAndInstall(store, keys, sources), /does not match a trusted key/)
  // A bundle that is not the announced version cannot downgrade: the held one stays.
  version = spec('0.8.1')
  await assert.rejects(checkAndInstall(store, keys, sources), /already held/)
  assert.deepEqual(store.newest()?.bundle.version, [0, 8, 1])
})
