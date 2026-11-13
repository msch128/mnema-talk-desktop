import assert from 'node:assert/strict'
import { randomBytes } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { afterEach, test } from 'node:test'
import { runInNewContext } from 'node:vm'
import { Jar } from '../src/core/cookies.ts'
import { PARTIAL_SUFFIX, downloadInto, downloadRequest, isPartialName, saveDownload } from '../src/core/download.ts'
import { REASONS, SENTENCES, cleanMessage, refusalMessage } from '../src/core/downloadtext.ts'
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
    assert.doesNotMatch(name, /^(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|COM[1-9]|LPT[1-9]) *(?:\.|$)/i, name)
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

const EXPORT = '/api/users/me/export/download'

test('a complete body becomes the file, byte for byte, and no temporary file is left', async () => {
  const body = randomBytes(3 << 20)
  const { instance, server } = await instanceFor(answer(200, { 'content-disposition': 'attachment; filename="mnema-talk-export.zip"' }, body.subarray(0, 1 << 20), body.subarray(1 << 20)))
  const dir = folder()
  const dest = join(dir, 'mnema-talk-export.zip')
  assert.deepEqual(await downloadInto(instance, EXPORT, dest), { ok: true, bytes: body.length })
  assert.ok(readFileSync(dest).equals(body))
  assert.deepEqual(readdirSync(dir), ['mnema-talk-export.zip'])
  // The request is the proxy's: the session cookie and the instance's own origin.
  assert.equal(server.requests[0]?.headers.cookie, 'session=s3cret')
  assert.equal(server.requests[0]?.headers.origin, server.origin.origin)
})

test('the place is chosen before the server is asked: a cancelled dialog leaves the export unspent', async () => {
  let spent = false
  const { instance, server } = await instanceFor((_request, res) => {
    res.writeHead(200, { 'content-disposition': 'attachment; filename="mnema-talk-export.zip"' })
    res.end(Buffer.from('PK small export'), () => { spent = true })
  })
  const proposed: string[] = []
  const cancelled = await saveDownload(instance, { path: EXPORT, name: 'mnema-talk-export.zip' }, (name) => { proposed.push(name); return Promise.resolve(null) })
  assert.deepEqual(cancelled, { cancelled: true })
  assert.deepEqual(proposed, ['mnema-talk-export.zip'])
  assert.equal(server.requests.length, 0)
  assert.equal(spent, false)
  const dir = folder()
  const chosen = join(dir, 'mine.zip')
  const done = await saveDownload(instance, { path: EXPORT, name: '' }, () => Promise.resolve(chosen))
  assert.deepEqual(done, { ok: true, bytes: 15, dest: chosen })
  assert.equal(readFileSync(chosen, 'utf8'), 'PK small export')
})

test('a folder that cannot be written asks the server for nothing and says why in plain words', async () => {
  const { instance, server } = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  const dir = folder()
  writeFileSync(join(dir, 'blocker'), 'a file where the folder should be')
  const result = await downloadInto(instance, EXPORT, join(dir, 'blocker', 'Downloads', 'x.zip'))
  assert.deepEqual(result, { ok: false, error: 'The file could not be saved: the folder does not exist. Check the folder, then try again.' })
  assert.equal(server.requests.length, 0)
})

test('a folder that does not exist yet is made and gets the file', async () => {
  const { instance } = await instanceFor(reply(200, {}, 'PK'))
  const dest = join(folder(), 'new', 'Downloads', 'x.zip')
  assert.deepEqual(await downloadInto(instance, EXPORT, dest), { ok: true, bytes: 2 })
  assert.equal(readFileSync(dest, 'utf8'), 'PK')
})

test('a connection cut in the middle of the body, or one shorter than its length, leaves nothing and says so', async () => {
  const cut = (headers: Record<string, string>, end: boolean): Parameters<typeof serve>[0] => (_request, res) => {
    res.writeHead(200, headers)
    res.write(Buffer.alloc(100, 1))
    setTimeout(() => (end ? res.end() : res.destroy()), 30)
  }
  for (const [headers, end] of [[{ 'content-length': '5000' }, false], [{}, false]] as [Record<string, string>, boolean][]) {
    const { instance } = await instanceFor(cut(headers, end))
    const dir = folder()
    assert.deepEqual(await downloadInto(instance, '/api/x', join(dir, 'x.bin')), { ok: false, error: SENTENCES.cut })
    assert.deepEqual(readdirSync(dir), [])
  }
})

test('a body that stalls is dropped after the idle time', async () => {
  const { instance } = await instanceFor((_request, res) => {
    res.writeHead(200, {})
    res.write(Buffer.alloc(10))
  })
  const dir = folder()
  assert.deepEqual(await downloadInto(instance, '/api/x', join(dir, 'x.bin'), { cap: 1 << 20, idleMs: 150 }), { ok: false, error: SENTENCES.cut })
  assert.deepEqual(readdirSync(dir), [])
})

test('a refusal writes nothing and names the reason and what to do: 404 is a spent file, else the server\'s message or the status', async () => {
  const json = { 'content-type': 'application/json' }
  const cases: [number, string, string][] = [
    [404, '{"error":{"code":"NOT_FOUND","message":"export not found"}}', SENTENCES.gone],
    [409, '{"error":{"code":"CONFLICT","message":"the export is not ready yet."}}', 'The server refused the download: the export is not ready yet. Try again later.'],
    [403, '{"message":"forbidden\\u0007 here"}', 'The server refused the download: forbidden here. Try again later.'],
    [500, 'not json', 'The download failed (HTTP 500). Try again later.'],
    [401, '{"error":{"code":"UNAUTHORIZED"}}', 'The download failed (HTTP 401). Try again later.'],
  ]
  for (const [status, body, expected] of cases) {
    const { instance } = await instanceFor(reply(status, json, body))
    const dir = folder()
    assert.deepEqual(await downloadInto(instance, EXPORT, join(dir, 'x.zip')), { ok: false, error: expected }, String(status))
    assert.deepEqual(readdirSync(dir), [])
  }
  assert.ok(SENTENCES.gone.includes('request a new one'))
  assert.equal(refusalMessage(502, null), 'The download failed (HTTP 502). Try again later.')
})

test('a body above the cap is refused, announced or not, and leaves nothing', async () => {
  const announced = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  const dir = folder()
  assert.deepEqual(await downloadInto(announced.instance, '/api/x', join(dir, 'x.bin'), { cap: 10, idleMs: 5000 }), { ok: false, error: SENTENCES.too_large })
  const streamed = await instanceFor(answer(200, {}, Buffer.alloc(8), Buffer.alloc(8), Buffer.alloc(8)))
  assert.deepEqual(await downloadInto(streamed.instance, '/api/x', join(dir, 'x.bin'), { cap: 20, idleMs: 5000 }), { ok: false, error: SENTENCES.too_large })
  assert.deepEqual(readdirSync(dir), [])
})

const partial = (c: string) => `.${c.repeat(32)}${PARTIAL_SUFFIX}`
const ageBy = (path: string, ms: number) => {
  const past = new Date(Date.now() - ms)
  utimesSync(path, past, past)
}

test('a download removes partial files left by an earlier run, and nothing a person named or another download still writes', async () => {
  const { instance } = await instanceFor(reply(200, {}, 'PK'))
  const dir = folder()
  const leftover = join(dir, partial('a'))
  const running = join(dir, partial('b'))
  const persons = ['.old.mnema-download', `.${'g'.repeat(32)}${PARTIAL_SUFFIX}`, 'a.mnema-download']
  for (const path of [leftover, running, ...persons.map((n) => join(dir, n))]) writeFileSync(path, 'x')
  for (const name of persons) ageBy(join(dir, name), 7 * 3600_000)
  ageBy(leftover, 7 * 3600_000)
  // Another server's download that has waited an hour for its first byte.
  ageBy(running, 3600_000)
  assert.deepEqual(await downloadInto(instance, '/api/media/x', join(dir, 'x.zip')), { ok: true, bytes: 2 })
  assert.deepEqual(readdirSync(dir).sort(), [...persons, partial('b'), 'x.zip'].sort())
  assert.equal(isPartialName(partial('0')), true)
  for (const name of persons) assert.equal(isPartialName(name), false, name)
})

test('a server that sends no answer ends the download after the idle time and leaves nothing', async () => {
  const { instance } = await instanceFor(() => { /* never answers */ })
  const dir = folder()
  const started = Date.now()
  assert.deepEqual(await downloadInto(instance, EXPORT, join(dir, 'x.zip'), { cap: 1 << 20, idleMs: 150 }), { ok: false, error: SENTENCES.unreachable })
  assert.ok(Date.now() - started < 3000)
  assert.deepEqual(readdirSync(dir), [])
})

test('two servers downloading into one folder keep each other\'s partial file', async () => {
  const late = await instanceFor((_request, res) => { setTimeout(() => { res.writeHead(200, {}); res.end('PK a') }, 400) })
  const quick = await instanceFor(reply(200, {}, 'PK b'))
  const dir = folder()
  const limits = { cap: 1 << 20, idleMs: 2000 }
  const a = downloadInto(late.instance, '/api/a', join(dir, 'a.zip'), limits)
  await new Promise((resolve) => setTimeout(resolve, 300))
  const b = await downloadInto(quick.instance, '/api/b', join(dir, 'b.zip'), limits)
  assert.deepEqual([await a, b], [{ ok: true, bytes: 4 }, { ok: true, bytes: 4 }])
  assert.deepEqual(readdirSync(dir).sort(), ['a.zip', 'b.zip'])
})

test('only /api/ of the server can be asked for, and nothing is written for anything else', async () => {
  const { instance, server } = await instanceFor(reply(200, {}, Buffer.alloc(100)))
  const dir = folder()
  for (const path of ['/index.html', '/api/../x', '//evil.example/api/x', 'https://evil.example/api/x', '/api/a\\b']) {
    assert.deepEqual(await downloadInto(instance, path, join(dir, 'x.bin')), { ok: false, error: SENTENCES.bad_path }, path)
    assert.deepEqual(await saveDownload(instance, { path, name: '' }, () => Promise.resolve(join(dir, 'x.bin'))), { ok: false, error: SENTENCES.bad_path }, path)
  }
  assert.equal(server.requests.length, 0)
  assert.equal(existsSync(join(dir, 'x.bin')), false)
})

test('the sentences, the reasons and the cleaning of a server message are the shared table', () => {
  const v = JSON.parse(readFileSync(new URL('../../test-vectors/proxy.json', import.meta.url), 'utf8')) as { download_errors: { sentences: Record<string, string>, reasons: Record<string, string>, messages: { input: string, output: string | null }[] } }
  assert.deepEqual(v.download_errors.sentences, SENTENCES)
  assert.deepEqual(v.download_errors.reasons, REASONS)
  for (const c of v.download_errors.messages) assert.equal(cleanMessage(c.input), c.output, JSON.stringify(c))
})
