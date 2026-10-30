/**
 * The community icon: what is accepted (the vectors the Rust host reads too,
 * `test-vectors/rail-icon.json`), where it is asked for, and how it rests.
 */
import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { crc32 } from 'node:zlib'
import { IconCache, MAX_ICON_BYTES, checkIcon, fetchIcon, iconToken, iconWanted, refreshIcon } from '../src/core/icon.ts'
import { reply, serve } from './helpers/server.ts'

type Json = any // eslint-disable-line @typescript-eslint/no-explicit-any -- vector files are untyped JSON
const vectors: Json = JSON.parse(readFileSync(new URL('../../test-vectors/rail-icon.json', import.meta.url), 'utf8'))

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-icon-')); dirs.push(d); return d }

const good = Buffer.from(vectors.cases[0].body, 'base64')
const ID = 'aaaaaaaaaaaaaaaa'

function chunk(type: string, data: Buffer): Buffer {
  const head = Buffer.alloc(4)
  head.writeUInt32BE(data.length)
  const tail = Buffer.alloc(4)
  tail.writeUInt32BE(crc32(Buffer.concat([Buffer.from(type), data])))
  return Buffer.concat([head, Buffer.from(type), data, tail])
}

/** The good png grown to `size` bytes with a text chunk before its end. */
function padded(size: number): Buffer {
  const end = good.length - 12
  const gap = size - good.length - 12
  return Buffer.concat([good.subarray(0, end), chunk('tEXt', Buffer.alloc(gap)), good.subarray(end)])
}

for (const c of vectors.cases as Json[]) {
  test(`icon vector: ${c.name}`, () => {
    const info = checkIcon(Buffer.from(c.body, 'base64'), c.content_type)
    if (!c.ok) return assert.equal(info, null)
    assert.deepEqual(info, { kind: c.kind, width: c.width, height: c.height })
  })
}

test('an icon of exactly one mebibyte is taken and one byte more is not', () => {
  assert.equal(MAX_ICON_BYTES, vectors.limit)
  assert.equal(padded(MAX_ICON_BYTES).length, MAX_ICON_BYTES)
  assert.notEqual(checkIcon(padded(MAX_ICON_BYTES), 'image/png'), null)
  assert.equal(checkIcon(padded(MAX_ICON_BYTES + 1), 'image/png'), null)
})

test('the health answer decides whether to ask: off means none, anything else means ask', () => {
  const wanted = (health: unknown) => iconWanted(Buffer.from(JSON.stringify(health)))
  for (const off of [false, null, '', 0]) assert.equal(wanted({ status: 'ok', community_icon: off }), false, String(off))
  for (const on of [true, 'v3', 1, {}]) assert.equal(wanted({ status: 'ok', community_icon: on }), true, String(on))
  assert.equal(wanted({ status: 'ok' }), true, 'an older server is asked once')
  assert.equal(iconWanted(Buffer.from('not json')), true)
})

test('the same bytes are the same version, other bytes another', () => {
  assert.match(iconToken(good), /^[0-9a-f]{16}$/)
  assert.equal(iconToken(good), iconToken(Buffer.from(good)))
  assert.notEqual(iconToken(good), iconToken(padded(good.length + 20)))
})

test('the icon is asked for at the server itself, with the etag it holds, and a png is taken', async () => {
  const server = await serve(reply(200, { 'content-type': 'image/png', etag: '"v1"' }, good))
  try {
    const got = await fetchIcon(server.origin, '"old"')
    assert.equal(got.status, 'changed')
    assert.equal(server.requests[0]!.url, '/api/community/icon')
    assert.equal(server.requests[0]!.headers['if-none-match'], '"old"')
    if (got.status === 'changed') assert.deepEqual([got.kind, got.etag, got.bytes.equals(good)], ['png', '"v1"', true])
  } finally { await server.close() }
})

test('not modified keeps, gone clears, and anything else leaves what is there', async () => {
  const answers: Record<number, string> = { 304: 'same', 404: 'none', 410: 'none', 500: 'failed', 403: 'failed' }
  for (const [status, expected] of Object.entries(answers)) {
    const server = await serve(reply(Number(status), {}, status === '304' ? '' : 'x'))
    try { assert.equal((await fetchIcon(server.origin, '"v"')).status, expected, status) } finally { await server.close() }
  }
})

test('a redirect is a failure, never followed', async () => {
  const away = await serve(reply(200, { 'content-type': 'image/png' }, good))
  const server = await serve(reply(302, { location: new URL('/api/community/icon', away.origin).href }, ''))
  try {
    assert.equal((await fetchIcon(server.origin, null)).status, 'failed')
    assert.equal(away.requests.length, 0)
  } finally { await server.close(); await away.close() }
})

test('a body over the limit, a wrong type and bytes that are not an image are failures', async () => {
  const cases: [string, Buffer][] = [['image/png', padded(MAX_ICON_BYTES + 1)], ['image/svg+xml', Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>')], ['image/png', Buffer.from('<html></html>')], ['text/plain', good]]
  for (const [type, body] of cases) {
    const server = await serve(reply(200, { 'content-type': type }, body))
    try { assert.equal((await fetchIcon(server.origin, null)).status, 'failed', type) } finally { await server.close() }
  }
  // A body that lies about its length is cut at the limit.
  const liar = await serve((_request, res) => { res.writeHead(200, { 'content-type': 'image/png' }); res.end(padded(MAX_ICON_BYTES + 1000)) })
  try { assert.equal((await fetchIcon(liar.origin, null)).status, 'failed') } finally { await liar.close() }
  assert.equal((await fetchIcon(new URL('http://127.0.0.1:1/'), null, 500)).status, 'failed')
})

test('an icon rests next to the other data of its server and is checked again as it is read', () => {
  const dir = tmp()
  const cache = new IconCache(dir)
  assert.equal(cache.read(ID), null)
  cache.store(ID, good, 'png', '"v1"')
  assert.deepEqual(readdirSync(join(dir, 'icons')).sort(), [`${ID}.etag`, `${ID}.png`])
  assert.equal(cache.etag(ID), '"v1"')
  assert.equal(cache.token(ID), iconToken(good))
  assert.equal(cache.dataUrl(ID), `data:image/png;base64,${good.toString('base64')}`)
  // Another format replaces it; no etag removes the old one.
  const webp = Buffer.from(vectors.cases.find((c: Json) => c.kind === 'webp').body, 'base64')
  cache.store(ID, webp, 'webp', null)
  assert.deepEqual(readdirSync(join(dir, 'icons')), [`${ID}.webp`])
  assert.match(cache.dataUrl(ID)!, /^data:image\/webp;base64,/)
  // A file that was damaged on disk is not shown.
  writeFileSync(join(dir, 'icons', `${ID}.webp`), 'garbage')
  assert.equal(cache.dataUrl(ID), null)
  cache.clear(ID)
  assert.deepEqual(readdirSync(join(dir, 'icons')), [])
  assert.equal(new IconCache(null).read(ID), null)
  assert.equal(cache.read('../../etc/passwd'), null)
})

test('icons of servers that are not in the list are swept', () => {
  const dir = tmp()
  const cache = new IconCache(dir)
  cache.store(ID, good, 'png', '"a"')
  cache.store('bbbbbbbbbbbbbbbb', good, 'png', null)
  mkdirSync(join(dir, 'icons'), { recursive: true })
  cache.sweep(new Set([ID]))
  assert.deepEqual(readdirSync(join(dir, 'icons')).sort(), [`${ID}.etag`, `${ID}.png`])
})

test('refreshing on connect stores a new icon, keeps it on 304, clears it on 404 and when the server turns it off', async () => {
  const dir = tmp()
  const cache = new IconCache(dir)
  let status = 200
  const server = await serve((request, res) => {
    if (status === 200 && request.headers['if-none-match'] === '"v1"') { res.writeHead(304); res.end(); return }
    reply(status, { 'content-type': 'image/png', etag: '"v1"' }, status === 200 ? good : 'gone')(request, res)
  })
  try {
    assert.equal(await refreshIcon(cache, ID, server.origin, true), true)
    assert.equal(cache.token(ID), iconToken(good))
    assert.equal(await refreshIcon(cache, ID, server.origin, true), false, 'not modified')
    assert.equal(server.requests.at(-1)!.headers['if-none-match'], '"v1"')
    status = 500
    assert.equal(await refreshIcon(cache, ID, server.origin, true), false)
    assert.notEqual(cache.token(ID), null, 'a failure keeps the icon')
    status = 404
    assert.equal(await refreshIcon(cache, ID, server.origin, true), true)
    assert.equal(cache.token(ID), null)
    status = 200
    await refreshIcon(cache, ID, server.origin, true)
    const asked = server.requests.length
    assert.equal(await refreshIcon(cache, ID, server.origin, false), true, 'switched off clears without asking')
    assert.equal(server.requests.length, asked)
    assert.equal(existsSync(join(dir, 'icons', `${ID}.png`)), false)
  } finally { await server.close() }
})
