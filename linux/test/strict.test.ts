import assert from 'node:assert/strict'
import { createPublicKey } from 'node:crypto'
import { test } from 'node:test'
import { verifyBundle } from '../src/core/bundle.ts'
import { bundleAsset } from '../src/core/updates.ts'
import { parseHealth } from '../src/core/client.ts'
import { isEd25519Point, publicKeyFromRaw, verifyEd25519 } from '../src/core/ed25519.ts'
import { lossyTrimmed } from '../src/core/encoding.ts'
import { JsonFloat, parseStrictJson } from '../src/core/json.ts'
import { verifyStatement } from '../src/core/trust.ts'
import { entries, entry, signWith, spec, statementText, testKey, trusted, zipEntries } from './helpers/fixtures.ts'
import { rootKey } from '../src/core/trust.ts'

const bytes = (text: string) => Buffer.from(text)
const hex = (text: string) => Buffer.from(text, 'hex')
const SMALL_ORDER = [
  '0100000000000000000000000000000000000000000000000000000000000000',
  'ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
  '0000000000000000000000000000000000000000000000000000000000000000',
  '0000000000000000000000000000000000000000000000000000000000000080',
  '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05',
  '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85',
  'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a',
  'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa',
]

test('a key of small order is not a key', () => {
  // The identity is on the curve, which is all the point check asks for.
  assert.equal(isEd25519Point(hex(SMALL_ORDER[0] as string)), true)
  for (const point of SMALL_ORDER) assert.equal(publicKeyFromRaw(hex(point)), null, point)
  assert.ok(publicKeyFromRaw(testKey(7).raw))
})

test('verification is strict: R of small order and S at or above the group order are refused', () => {
  const key = testKey(7)
  const message = bytes('any message')
  const good = signWith(key, message)
  assert.equal(verifyEd25519(publicKeyFromRaw(key.raw) as never, message, good), true)
  // S plus the group order is the same scalar written non-canonically.
  const L = (1n << 252n) + 27742317777372353535851937790883648493n
  let s = 0n
  for (let i = 63; i >= 32; i--) s = (s << 8n) | BigInt(good[i] as number)
  const widened = Buffer.from(good)
  let t = s + L
  for (let i = 32; i < 64; i++) { widened[i] = Number(t & 0xffn); t >>= 8n }
  assert.equal(verifyEd25519(publicKeyFromRaw(key.raw) as never, message, widened), false)
  // The identity as key, R = identity and S = 0 verifies every message under the plain check.
  const identity = createPublicKey({ key: Buffer.concat([hex('302a300506032b6570032100'), hex(SMALL_ORDER[0] as string)]), format: 'der', type: 'spki' })
  const forged = Buffer.concat([hex(SMALL_ORDER[0] as string), Buffer.alloc(32)])
  assert.equal(verifyEd25519(identity, message, forged), false)
})

test('the strict reader refuses what serde refuses', () => {
  assert.equal(JSON.stringify(parseStrictJson(bytes('{"a":[1,true,null,"x\\u00e9"],"b":{"c":-5}}'))), '{"a":[1,true,null,"xé"],"b":{"c":-5}}')
  assert.throws(() => parseStrictJson(Buffer.from('\uFEFF{}')), 'a byte order mark')
  assert.throws(() => parseStrictJson(bytes('{"a":1,"a":2}')), 'a repeated key')
  assert.throws(() => parseStrictJson(bytes('{"a":{"b":1,"b":1}}')), 'a repeated nested key')
  for (const bad of ['', '{', '[1,]', '{"a":1,}', '01', '+1', '1.', '.5', '"\u0001"', '"\\ud800"', '"\\x"', '{"a" 1}', 'nul', '{} {}', 'NaN']) {
    assert.throws(() => parseStrictJson(bytes(bad)), bad)
  }
  assert.throws(() => parseStrictJson(Buffer.from([0x22, 0xff, 0x22])), 'invalid UTF-8')
  assert.throws(() => parseStrictJson(bytes('['.repeat(200) + ']'.repeat(200))), 'too deep')
  // Numbers: integers are numbers, everything else is not, big integers stay exact.
  assert.equal(parseStrictJson(bytes('1')), 1)
  assert.ok(parseStrictJson(bytes('1.0')) instanceof JsonFloat)
  assert.ok(parseStrictJson(bytes('1e0')) instanceof JsonFloat)
  assert.equal(parseStrictJson(bytes('9007199254740993')), 9007199254740993n)
  assert.equal(Object.getPrototypeOf(parseStrictJson(bytes('{"__proto__":1}'))), null)
})

function signedManifest(text: string) {
  const s = spec('0.8.0')
  const list = entries(s)
  const body = Buffer.from(text)
  entry(list, 'manifest.json').body = body
  entry(list, 'manifest.sig').body = Buffer.from(signWith(testKey(s.seed), body).toString('base64'))
  return zipEntries(list)
}

test('a signed manifest is read as strictly as the Tauri host reads it', () => {
  const s = spec('0.8.0')
  const original = entry(entries(s), 'manifest.json').body.toString('utf8')
  assert.doesNotThrow(() => verifyBundle(signedManifest(original), trusted(7)))
  const refused = (text: string, why: string) => assert.throws(() => verifyBundle(signedManifest(text), trusted(7)), /unreadable manifest/, why)
  refused(`\uFEFF${original}`, 'byte order mark')
  refused(`{"schema":1,${original.slice(1)}`, 'repeated key')
  refused(original.replace('"schema":1', '"schema":1.0'), 'schema written as 1.0')
  refused(original.replace('"web_client_api":1', '"web_client_api":1e0'), 'api written as 1e0')
  refused(original.replace(/"size":(\d+)/, '"size":$1.0'), 'size written as 1.0')
})

test('a trust statement is read strictly, and a signature is trimmed like Rust trims it', () => {
  const root = testKey(3)
  const text = statementText({})
  const sign = (doc: string) => Buffer.from(signWith(root, Buffer.from(doc)).toString('base64'))
  assert.ok(verifyStatement(Buffer.from(text), sign(text), rootKey(root.hex)))
  const dup = `{"serial":1,${text.slice(1)}`
  assert.throws(() => verifyStatement(Buffer.from(dup), sign(dup), rootKey(root.hex)), /unreadable/)
  const float = text.replace('"serial":1', '"serial":1.0')
  assert.throws(() => verifyStatement(Buffer.from(float), sign(float), rootKey(root.hex)), /unreadable/)
  // U+0085 is white space to Rust, U+FEFF is not.
  assert.ok(verifyStatement(Buffer.from(text), Buffer.concat([bytes('\u0085'), sign(text), bytes(' \n')]), rootKey(root.hex)))
  assert.throws(() => verifyStatement(Buffer.from(text), Buffer.concat([bytes('\uFEFF'), sign(text)]), rootKey(root.hex)), /malformed/)
  assert.equal(lossyTrimmed(bytes(' 　 x  ')), 'x')
})

test('health and release documents are read strictly', () => {
  assert.equal(parseHealth(bytes('{"status":"ok","version":"1.2.3","web_client_api":2}')).webClientApi, 2)
  for (const doc of ['\uFEFF{"status":"ok","web_client_api":2}', '{"status":"ok","status":"ok","web_client_api":2}', '{"status":"ok","web_client_api":2.0}']) {
    assert.throws(() => parseHealth(Buffer.from(doc)), /update/, doc)
  }
  const release = (asset: string) => `{"tag_name":"v1.2.3","assets":[${asset}]}`
  const ok = '{"name":"mnema-web-1.2.3.zip","browser_download_url":"https://github.com/x/y/releases/download/v1.2.3/mnema-web-1.2.3.zip","size":10}'
  assert.ok(bundleAsset(bytes(release(ok))))
  assert.equal(bundleAsset(bytes(release(ok.replace('"size":10', '"size":10.0')))), null)
  assert.equal(bundleAsset(bytes(`\uFEFF${release(ok)}`)), null)
  // An integer above 2^53 is a valid u64: the document is read, the asset is just too large.
  const huge = ok.replace('"size":10', '"size":18446744073709551615').replace('1.2.3.zip', '1.2.3.zip.big')
  assert.ok(bundleAsset(bytes(release(`${huge},${ok}`))))
  assert.equal(bundleAsset(bytes(release(ok.replace('"size":10', '"size":18446744073709551616')))), null)
})
