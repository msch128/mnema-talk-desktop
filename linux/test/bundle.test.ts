import assert from 'node:assert/strict'
import { test } from 'node:test'
import { MAX_ENTRIES, MAX_FILE, MAX_MANIFEST, MAX_PATH, parseKey, validPath, verifyBundle } from '../src/core/bundle.ts'
import { isEd25519Point } from '../src/core/ed25519.ts'
import { compareVersions, formatVersion, parseVersion } from '../src/core/version.ts'
import { bundle, entries, entry, resigned, spec, testKey, trusted, zipEntries, zipOf, type Entry } from './helpers/fixtures.ts'
import { PINNED_BUNDLE_KEYS, ROOT_PUB } from '../src/core/pinned.ts'

test('the pinned key is well formed and matches its published id', () => {
  assert.equal(PINNED_BUNDLE_KEYS.length, 1)
  assert.equal(parseKey(PINNED_BUNDLE_KEYS[0] as string)?.id, '1069c76ff2e11272')
  assert.equal(parseKey('00'), null)
  assert.equal(parseKey('zz'), null)
})

test('ed25519 points are told from other 32 byte strings', () => {
  const real = ['d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a', '3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c', ...PINNED_BUNDLE_KEYS, ROOT_PUB, testKey(7).hex]
  for (const hex of real) assert.equal(isEd25519Point(Buffer.from(hex, 'hex')), true, hex)
  // y = 2 has no x on the curve.
  assert.equal(isEd25519Point(Buffer.from('0200000000000000000000000000000000000000000000000000000000000000', 'hex')), false)
  assert.equal(isEd25519Point(Buffer.alloc(31)), false)
})

test('versions parse strictly and order numerically', () => {
  assert.deepEqual(parseVersion('0.7.16'), [0, 7, 16])
  for (const bad of ['', '1', '1.2', '1.2.3.4', 'v1.2.3', '01.2.3', '1.2.x', '1..3', '1.2.3-rc1', '-1.0.0', '99999999999.0.0', '+1.0.0']) {
    assert.equal(parseVersion(bad), null, bad)
  }
  assert.ok(compareVersions([0, 7, 9], [0, 7, 10]) < 0)
  assert.ok(compareVersions([0, 9, 0], [1, 0, 0]) < 0)
  assert.equal(formatVersion([1, 2, 3]), '1.2.3')
})

test('path rules match the signing tool', () => {
  for (const ok of ['index.html', 'assets/a-b.js', '\u00fcn\u00ef/x.js']) assert.ok(validPath(ok), ok)
  for (const bad of ['', '/abs', 'a//b', 'a/./b', 'a/../b', '..', '.', 'a/', 'a\\b', 'a\0b', 'a\u007f', 'a\nb', 'a'.repeat(MAX_PATH + 1)]) {
    assert.ok(!validPath(bad), JSON.stringify(bad))
  }
})

test('a bundle signed by a trusted key verifies and serves its files', () => {
  const b = verifyBundle(bundle(spec('0.8.0')), trusted(7))
  assert.deepEqual(b.version, [0, 8, 0])
  assert.equal(b.webClientApi, 1)
  assert.equal(Buffer.from(b.file('index.html') as Uint8Array).toString(), '<html>app</html>')
  assert.equal(b.file('missing'), undefined)
  assert.equal(b.commit, 'test')
  assert.equal(b.keyId, testKey(7).id)
})

test('a bundle from another key or with no keys is refused', () => {
  const data = bundle(spec('0.8.0'))
  assert.throws(() => verifyBundle(data, trusted(8)), /signature does not match a trusted key/)
  assert.throws(() => verifyBundle(data, []), /signature does not match a trusted key/)
})

function damaged(edit: (list: Entry[]) => void): () => unknown {
  const list = entries(spec('0.8.0'))
  edit(list)
  return () => verifyBundle(zipEntries(list), trusted(7))
}

test('tampering with any part of a signed bundle is refused', () => {
  assert.throws(damaged((e) => { entry(e, 'app/index.html').body = Buffer.from('<html>evil</html>') }))
  assert.throws(damaged((e) => { e.push({ name: 'app/evil.js', body: Buffer.from('x') }) }))
  assert.throws(damaged((e) => { e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1) }))
  assert.throws(damaged((e) => {
    e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1)
    e.push({ name: 'app/assets/other.js', body: Buffer.from('console.log(1)') })
  }))
  assert.throws(damaged((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.json'), 1) }), /manifest or signature is missing/)
  assert.throws(damaged((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.sig'), 1) }), /manifest or signature is missing/)
  assert.throws(damaged((e) => { const m = entry(e, 'manifest.json'); m.body = Buffer.from(m.body.toString().replace('0.8.0', '9.9.9')) }))
  assert.throws(damaged((e) => { entry(e, 'manifest.sig').body = Buffer.from('***') }), /malformed signature/)
  assert.throws(damaged((e) => { entry(e, 'manifest.sig').body = Buffer.from('AAAA') }), /malformed signature/)
  assert.throws(damaged((e) => { entry(e, 'manifest.json').body = Buffer.alloc(MAX_MANIFEST + 1, 0x20) }), /manifest.json: too large/)
  assert.throws(damaged((e) => { entry(e, 'manifest.sig').body = Buffer.alloc(2000, 0x41) }), /manifest.sig: too large/)
  assert.throws(() => verifyBundle(Buffer.from('PK not a zip'), trusted(7)), /not a readable zip/)
})

test('a correctly signed manifest still has to be well formed', () => {
  assert.doesNotThrow(() => verifyBundle(resigned(() => {}), trusted(7)))
  const refused = (edit: (m: Record<string, unknown>) => void) => assert.throws(() => verifyBundle(resigned(edit), trusted(7)))
  for (const [field, value] of [['schema', 2], ['name', 'other'], ['key_id', '0000000000000000'], ['version', '1'], ['web_client_api', 0], ['min_server_version', '9.0.0'], ['security_floor', '9.0.0'], ['files', []]] as const) {
    refused((m) => { m[field] = value })
  }
  refused((m) => { m.surprise = 1 })
  const files = (m: Record<string, unknown>) => m.files as Record<string, unknown>[]
  refused((m) => { (files(m)[0] as Record<string, unknown>).path = '../x' })
  refused((m) => { (files(m)[1] as Record<string, unknown>).path = (files(m)[0] as Record<string, unknown>).path })
  refused((m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'zz' })
  refused((m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'A'.repeat(64) })
  refused((m) => { (files(m)[0] as Record<string, unknown>).size = MAX_FILE + 1 })
  refused((m) => { (files(m)[0] as Record<string, unknown>).size = 1 })
  refused((m) => { (files(m)[0] as Record<string, unknown>).sha256 = '0'.repeat(64) })
  refused((m) => { (files(m)[0] as Record<string, unknown>).path = 'home.html' })
  refused((m) => {
    const list = files(m)
    for (let i = 0; i < 5; i++) list.push({ path: `big${i}`, sha256: '0'.repeat(64), size: MAX_FILE })
    for (const f of list) f.size = MAX_FILE
  })
  refused((m) => { for (let i = 0; i <= MAX_ENTRIES; i++) files(m).push({ path: `f${i}`, sha256: '0'.repeat(64), size: 0 }) })
})

test('too many zip entries are refused before anything is read', () => {
  const list = entries(spec('0.8.0'))
  for (let i = 0; i < MAX_ENTRIES + 1; i++) list.push({ name: `app/pad${i}`, body: Buffer.alloc(0) })
  assert.throws(() => verifyBundle(zipEntries(list), trusted(7)), /^BundleError: too many entries$|too many entries/)
})

test('duplicate names, bad names, encryption and lying headers are refused', () => {
  const base = entries(spec('0.8.0'))
  const items = base.map((e) => ({ name: e.name, body: e.body }))
  assert.throws(() => verifyBundle(zipOf([...items, { name: 'app/index.html', body: Buffer.from('x') }]), trusted(7)), /duplicate entry app\/index.html/)
  assert.throws(() => verifyBundle(zipOf(items.map((i) => (i.name === 'app/index.html' ? { ...i, crc: 1 } : i))), trusted(7)), /index.html: damaged/)
  assert.throws(() => verifyBundle(zipOf(items.map((i) => (i.name === 'app/index.html' ? { ...i, flags: 1 } : i))), trusted(7)), /unreadable entry/)
  assert.throws(() => verifyBundle(zipOf(items.map((i) => (i.name === 'app/index.html' ? { ...i, declaredSize: 3 } : i))), trusted(7)), /size does not match the manifest/)
  // Deflated entries are read like stored ones.
  assert.doesNotThrow(() => verifyBundle(zipOf(items.map((i) => ({ ...i, method: 8 as const }))), trusted(7)))
  const compressible = entries(spec('0.8.0', { files: [['index.html', 'x'.repeat(100_000)]] })).map((e) => ({ name: e.name, body: e.body, method: 8 as const, level: 9 }))
  assert.doesNotThrow(() => verifyBundle(zipOf(compressible), trusted(7)))
})

test('a deflated entry that inflates past its declared size is refused', () => {
  const base = entries(spec('0.8.0'))
  const big = Buffer.alloc(4096, 0x61)
  const items = base.map((e) => ({ name: e.name, body: e.body }))
  const bomb = items.map((i) => (i.name === 'app/index.html' ? { name: i.name, body: big, method: 8 as const, declaredSize: 16 } : i))
  assert.throws(() => verifyBundle(zipOf(bomb), trusted(7)))
})
