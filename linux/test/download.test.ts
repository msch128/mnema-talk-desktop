import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { afterEach, test } from 'node:test'
import { runInNewContext } from 'node:vm'
import { Jar } from '../src/core/cookies.ts'
import { INTERRUPTED, NOTHING_TO_DOWNLOAD, TOO_LARGE, downloadRequest, openDownload, refusalMessage } from '../src/core/download.ts'
import { MAX_NAME_BYTES, downloadName, inFolder } from '../src/core/downloadname.ts'
import { Limits, type Instance } from '../src/core/proxy.ts'
import { reply, serve, type TestServer } from './helpers/server.ts'

const servers: TestServer[] = []
const dirs: string[] = []
afterEach(async () => {
  for (const s of servers.splice(0)) await s.close()
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

const folder = (): string => {
  const d = mkdtempSync(join(tmpdir(), 'mnema-download-'))
  dirs.push(d)
  return d
}

async function instanceFor(handler: Parameters<typeof serve>[0]): Promise<{ instance: Instance, server: TestServer }> {
  const server = await serve(handler)
  servers.push(server)
  const jar = new Jar(null, null, 0)
  jar.store(['session=s3cret; Max-Age=3600'], false, Math.floor(Date.now() / 1000))
  return { instance: { origin: server.origin, jar, limits: new Limits(), now: () => Math.floor(Date.now() / 1000) }, server }
}

// ------------------------------------------------------------------ names

test('name vectors: the disposition, the attribute and the address, cut down to a plain name', () => {
  const v = JSON.parse(readFileSync(new URL('../../test-vectors/proxy.json', import.meta.url), 'utf8')) as { download_names: { disposition: string | null, attribute: string, path: string, name: string }[] }
  assert.ok(v.download_names.length >= 10)
  for (const c of v.download_names) assert.equal(downloadName(c.disposition, c.attribute, c.path), c.name, JSON.stringify(c))
})

test('every proposed name is one plain component: no separator, reserved character, device, hidden or over-long name', () => {
  const v = JSON.parse(readFileSync(new URL('../../test-vectors/proxy.json', import.meta.url), 'utf8')) as { download_names: { name: string }[] }
  for (const { name } of v.download_names) {
    assert.ok(Buffer.byteLength(name) <= MAX_NAME_BYTES, name)
    assert.doesNotMatch(name, /[/\\<>:"|?*]/, name)
    assert.ok([...name].every((c) => { const code = c.codePointAt(0) as number; return code >= 0x20 && (code < 0x7f || code > 0x9f) }), name)
    assert.doesNotMatch(name, /^[ .]|[ .]$/, name)
    assert.doesNotMatch(name, /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9]) *(?:\.|$)/i, name)
    assert.equal(dirname(inFolder('/dl', name)), resolve('/dl'), name)
  }
})

test('a name that would leave the folder is saved as "download" in it', () => {
  assert.equal(inFolder('/dl', 'a.zip'), resolve('/dl', 'a.zip'))
  for (const bad of ['../a.zip', 'sub/a.zip', '..', '.', '', '/etc/passwd']) {
    const path = inFolder('/dl', bad)
    assert.equal(dirname(path), resolve('/dl'), bad)
    assert.equal(basename(path), 'download', bad)
  }
})

// -------------------------------------------------------------- the sender

test('only the instance page of a server may ask for a download, and only for the API', () => {
  const ok = { path: '/api/users/me/export/download', name: 'x.zip' }
  assert.deepEqual(downloadRequest('instance', 'abc', ok), ok)
  for (const role of ['selector', 'attachment', 'updater', 'picker', 'rail', 'dialog', undefined]) {
    assert.throws(() => downloadRequest(role, 'abc', ok), /Not allowed/, String(role))
  }
  assert.throws(() => downloadRequest('instance', null, ok), /Not allowed/)
  for (const bad of [{ path: '/index.html', name: '' }, { path: 5, name: '' }, { path: '/api/x', name: 5 }, { path: `/api/${'a'.repeat(9000)}`, name: '' }, { path: '/api/x', name: 'n'.repeat(2000) }, {}]) {
    assert.throws(() => downloadRequest('instance', 'abc', bad), /invalid arguments/, JSON.stringify(bad).slice(0, 40))
  }
})

// ------------------------------------------------------------ the click

interface Anchor { href: string, getAttribute(name: string): string | null }

/** Runs the real preload against a stand-in page and returns what a click does. */
function clicking(role: string | null) {
  const invoked: unknown[][] = []
  let listener: ((event: unknown) => void) | null = null
  const electron = {
    contextBridge: { exposeInMainWorld() {} },
    ipcRenderer: { on() {}, sendSync: () => (role === null ? null : { role }), invoke: (...args: unknown[]) => { invoked.push(JSON.parse(JSON.stringify(args)) as unknown[]); return Promise.resolve({ ok: true, value: null }) } },
  }
  const code = stripTypeScriptTypes(readFileSync(new URL('../src/preload/preload.cts', import.meta.url), 'utf8'))
  const document = { addEventListener: (type: string, fn: (event: unknown) => void, capture: boolean) => { if (type === 'click' && capture) listener = fn } }
  runInNewContext(code, { require: () => electron, document, location: { href: 'mnema://localhost/c/1', protocol: 'mnema:', host: 'localhost' }, URL, Reflect, Promise, Object })
  return {
    invoked,
    registered: () => listener !== null,
    click(anchor: Anchor | null, extra: Record<string, unknown> = {}): boolean {
      let prevented = false
      listener?.({ button: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, defaultPrevented: false, target: { closest: () => anchor }, preventDefault: () => { prevented = true }, ...extra })
      return prevented
    },
  }
}

const link = (href: string, download: string | null = ''): Anchor => ({ href, getAttribute: (name) => (name === 'download' ? download : null) })

test('a download link of the app API is taken over, anything else is left to the page', () => {
  const page = clicking('instance')
  assert.equal(page.click(link('mnema://localhost/api/users/me/export/download')), true)
  assert.deepEqual(page.invoked, [['mnema:invoke', 'desktop_download', { path: '/api/users/me/export/download', name: '' }]])
  assert.equal(page.click(link('mnema://localhost/api/media/a/file.png?x=1', 'photo.png')), true)
  assert.deepEqual(page.invoked[1], ['mnema:invoke', 'desktop_download', { path: '/api/media/a/file.png?x=1', name: 'photo.png' }])
  const before = page.invoked.length
  for (const href of ['blob:mnema://localhost/1b2c', 'https://localhost/api/x', 'mnema://other/api/x', 'mnema://localhost/c/1', 'mnema://localhost/apix', 'data:text/plain,hi', 'http://[bad']) {
    assert.equal(page.click(link(href)), false, href)
  }
  for (const extra of [{ defaultPrevented: true }, { button: 1 }, { button: 2 }, { ctrlKey: true }, { shiftKey: true }, { altKey: true }, { metaKey: true }]) {
    assert.equal(page.click(link('mnema://localhost/api/x'), extra), false, JSON.stringify(extra))
  }
  assert.equal(page.click(null), false)
  assert.equal(page.invoked.length, before)
})

test('only an instance page listens for clicks', () => {
  assert.equal(clicking('instance').registered(), true)
  assert.equal(clicking('selector').registered(), false)
  assert.equal(clicking(null).registered(), false)
})

// -------------------------------------------------------------- the saver

const answer = (status: number, headers: Record<string, string>, ...pieces: Buffer[]): Parameters<typeof serve>[0] => (_request, res) => {
  res.writeHead(status, headers)
  for (const piece of pieces) res.write(piece)
  res.end()
}

test('a complete body becomes the file, byte for byte, and no temporary file is left', async () => {
  const body = randomBytes(3 << 20)
  const { instance, server } = await instanceFor(answer(200, { 'content-disposition': 'attachment; filename="mnema-talk-export.zip"' }, body.subarray(0, 1 << 20), body.subarray(1 << 20)))
  const opened = await openDownload(instance, '/api/users/me/export/download', '')
  assert.ok('pending' in opened)
  assert.equal(opened.pending.name, 'mnema-talk-export.zip')
  const dir = folder()
  const dest = join(dir, opened.pending.name)
  assert.deepEqual(await opened.pending.save(dest), { ok: true, bytes: body.length })
  assert.ok(readFileSync(dest).equals(body))
  assert.deepEqual(readdirSync(dir), ['mnema-talk-export.zip'])
  // The request is the proxy's: the session cookie and the instance's own origin.
  assert.equal(server.requests[0]?.headers.cookie, 'session=s3cret')
  assert.equal(server.requests[0]?.headers.origin, server.origin.origin)
})

test('a connection cut in the middle of the body leaves nothing and says so', async () => {
  const cut = (headers: Record<string, string>): Parameters<typeof serve>[0] => (_request, res) => {
    res.writeHead(200, headers)
    res.write(Buffer.alloc(100, 1))
    setTimeout(() => res.destroy(), 30)
  }
  for (const headers of [{ 'content-length': '5000' }, {}] as Record<string, string>[]) {
    const { instance } = await instanceFor(cut(headers))
    const opened = await openDownload(instance, '/api/x', 'x.bin')
    assert.ok('pending' in opened)
    const dir = folder()
    const result = await opened.pending.save(join(dir, 'x.bin'))
    assert.deepEqual(result, { ok: false, error: INTERRUPTED })
    assert.deepEqual(readdirSync(dir), [])
  }
})

test('a body that stalls is dropped after the idle time', async () => {
  const { instance } = await instanceFor((_request, res) => {
    res.writeHead(200, {})
    res.write(Buffer.alloc(10))
  })
  const opened = await openDownload(instance, '/api/x', 'x.bin', { cap: 1 << 20, idleMs: 150 })
  assert.ok('pending' in opened)
  const dir = folder()
  assert.deepEqual(await opened.pending.save(join(dir, 'x.bin')), { ok: false, error: INTERRUPTED })
  assert.deepEqual(readdirSync(dir), [])
})

test('a refusal writes nothing and names the reason: 404 is a spent file, else the server\'s message or the status', async () => {
  const dir = folder()
  const json = { 'content-type': 'application/json' }
  const cases: [number, string, string][] = [
    [404, '{"error":{"code":"NOT_FOUND","message":"export not found"}}', NOTHING_TO_DOWNLOAD],
    [409, '{"error":{"code":"CONFLICT","message":"the export is not ready yet"}}', 'the export is not ready yet'],
    [403, '{"message":"forbidden here"}', 'forbidden here'],
    [500, 'not json', 'The download failed (HTTP 500).'],
    [401, '{"error":{"code":"UNAUTHORIZED"}}', 'The download failed (HTTP 401).'],
  ]
  for (const [status, body, expected] of cases) {
    const { instance } = await instanceFor(reply(status, json, body))
    const opened = await openDownload(instance, '/api/users/me/export/download', 'x.zip')
    assert.deepEqual(opened, { error: expected }, String(status))
  }
  assert.deepEqual(readdirSync(dir), [])
  assert.equal(refusalMessage(404, null), NOTHING_TO_DOWNLOAD)
  assert.equal(refusalMessage(502, null), 'The download failed (HTTP 502).')
})

test('a body above the cap is refused, announced or not, and leaves nothing', async () => {
  const dir = folder()
  const announced = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  assert.deepEqual(await openDownload(announced.instance, '/api/x', '', { cap: 10, idleMs: 5000 }), { error: TOO_LARGE })
  const streamed = await instanceFor(answer(200, {}, Buffer.alloc(8), Buffer.alloc(8), Buffer.alloc(8)))
  const opened = await openDownload(streamed.instance, '/api/x', '', { cap: 20, idleMs: 5000 })
  assert.ok('pending' in opened)
  assert.deepEqual(await opened.pending.save(join(dir, 'x.bin')), { ok: false, error: TOO_LARGE })
  assert.deepEqual(readdirSync(dir), [])
})

test('a disk problem is reported with its reason and leaves nothing', async () => {
  const { instance } = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  const opened = await openDownload(instance, '/api/x', 'x.bin')
  assert.ok('pending' in opened)
  const dir = folder()
  const result = await opened.pending.save(join(dir, 'missing', 'x.bin'))
  assert.deepEqual(result, { ok: false, error: 'The file could not be saved: the folder does not exist.' })
  assert.deepEqual(readdirSync(dir), [])
})

test('an abandoned download writes nothing, and only /api/ of the server can be asked for', async () => {
  const { instance, server } = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  const opened = await openDownload(instance, '/api/x', 'x.bin')
  assert.ok('pending' in opened)
  opened.pending.abandon()
  const before = server.requests.length
  for (const path of ['/index.html', '/api/../x', '//evil.example/api/x', 'https://evil.example/api/x', '/api/a\\b']) {
    const refused = await openDownload(instance, path, '')
    assert.ok('error' in refused, path)
  }
  assert.equal(server.requests.length, before)
  assert.equal(existsSync(join(folder(), 'x.bin')), false)
})
