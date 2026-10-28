import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { verifyBundle } from '../src/core/bundle.ts'
import { Jar, MAX_COOKIES, type Vault } from '../src/core/cookies.ts'
import { APP_CSP, LEGACY_APP_CSP, appCsp, LARGE_BODY_BUDGET, Limits, MAX_BODY, SMALL_BODY, forward, inlineSafe, mimeFor, payloadTooLarge, serveBundle, target, type Instance, type Resp } from '../src/core/proxy.ts'
import { bundle, spec, trusted } from './helpers/fixtures.ts'
import { reply, serve, type TestServer } from './helpers/server.ts'

const servers: TestServer[] = []
const dirs: string[] = []
afterEach(async () => {
  for (const s of servers.splice(0)) await s.close()
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

function tmp(): string {
  const d = mkdtempSync(join(tmpdir(), 'mnema-proxy-'))
  dirs.push(d)
  return d
}

const header = (resp: Resp, name: string) => resp.headers.find(([n]) => n === name)?.[1]

const served = verifyBundle(bundle(spec('0.8.0', { files: [['index.html', '<html>app</html>'], ['assets/app-1.js', 'console.log(1)'], ['logo.svg', '<svg/>'], ['sw.js', 'x']] })), trusted(7))

test('the bundle is served with the app policy, the shell for client-side routes and 404 for missing assets', () => {
  const index = serveBundle(served, 'GET', '/')
  assert.equal(index.status, 200)
  assert.equal(Buffer.from(index.body).toString(), '<html>app</html>')
  assert.equal(header(index, 'content-security-policy'), LEGACY_APP_CSP)
  assert.match(APP_CSP, /script-src 'self' 'wasm-unsafe-eval'/)
  assert.equal(header(index, 'x-content-type-options'), 'nosniff')
  assert.equal(header(index, 'cache-control'), 'no-cache')
  const asset = serveBundle(served, 'GET', '/assets/app-1.js')
  assert.equal(header(asset, 'content-type'), 'text/javascript; charset=utf-8')
  assert.equal(header(asset, 'cache-control'), 'public, max-age=31536000, immutable')
  assert.equal(serveBundle(served, 'GET', '/assets/missing.js').status, 404)
  const route = serveBundle(served, 'GET', '/c/11111111-1111-1111-1111-111111111111')
  assert.equal(route.status, 200)
  assert.equal(header(route, 'content-type'), 'text/html; charset=utf-8')
  assert.equal(serveBundle(served, 'GET', '//index.html').status, 200)
  const head = serveBundle(served, 'HEAD', '/')
  assert.equal(head.body.length, 0)
  const post = serveBundle(served, 'POST', '/')
  assert.equal(post.status, 405)
  assert.equal(header(post, 'allow'), 'GET, HEAD')
  assert.equal(mimeFor('a.WASM'), 'application/wasm')
  assert.equal(mimeFor('noext'), 'application/octet-stream')
})

test('bundles from 1.1.0 get the strict style policy, older ones keep inline styles', () => {
  const files: [string, string][] = [['index.html', '<html>app</html>']]
  const csp = (version: string) => header(serveBundle(verifyBundle(bundle(spec(version, { files })), trusted(7)), 'GET', '/'), 'content-security-policy')
  for (const version of ['1.1.0', '1.1.1', '1.2.0', '2.0.0']) {
    assert.equal(csp(version), APP_CSP, version)
  }
  for (const version of ['0.8.0', '1.0.0', '1.0.1', '1.0.99']) {
    assert.equal(csp(version), LEGACY_APP_CSP, version)
  }
  assert.match(APP_CSP, /style-src 'self'; style-src-attr 'none';/)
  assert.doesNotMatch(APP_CSP, /unsafe-inline/)
  assert.match(LEGACY_APP_CSP, /style-src 'self' 'unsafe-inline'/)
  // Only the style sources differ.
  assert.equal(APP_CSP.replace("style-src 'self'; style-src-attr 'none'", 'S'), LEGACY_APP_CSP.replace("style-src 'self' 'unsafe-inline'", 'S'))
  assert.equal(appCsp([1, 1, 0]), APP_CSP)
  assert.equal(appCsp([1, 0, 9]), LEGACY_APP_CSP)
})

test('only /api/ of the instance is reachable, whatever the path contains', () => {
  const origin = new URL('https://community.example')
  assert.equal(target(origin, '/api/channels?x=1')?.href, 'https://community.example/api/channels?x=1')
  for (const bad of ['/index.html', '/api', '/api/../x', '/api/%2e%2e/x', '/api/a\\b', '/api/a\nb', '//evil.example/api/x', '/apix', 'https://evil.example/api/x', '/api/\u0085']) {
    assert.equal(target(origin, bad), null, JSON.stringify(bad))
  }
})

test('media types that could be documents or scripts are not inline safe', () => {
  for (const ok of ['application/json', 'text/plain; charset=utf-8', 'image/png', 'IMAGE/JPEG', 'audio/ogg', 'video/mp4', 'application/vnd.api+json', 'application/octet-stream']) {
    assert.ok(inlineSafe(ok), ok)
  }
  for (const bad of ['text/html', 'image/svg+xml', 'application/xml', 'text/javascript', 'application/javascript', 'application/pdf', 'text/xml', 'application/xhtml+xml', '', 'text/css']) {
    assert.ok(!inlineSafe(bad), bad)
  }
})

test('the jar accepts only what a browser would for its prefixes, secure flag and size', () => {
  const jar = new Jar(null, null, 0)
  jar.store([
    '__Host-a=1; Max-Age=60', // no Secure
    '__Host-b=1; Secure; Path=/x; Max-Age=60', // wrong path
    '__Host-c=1; Secure; Path=/; Domain=example.org; Max-Age=60', // Domain
    '__Secure-d=1; Max-Age=60',
    'e=1; Secure; Max-Age=60', // Secure over HTTP
    '=nameless; Max-Age=60',
    'not a cookie',
  ], false, 0)
  assert.equal(jar.header(1), null)
  jar.store(['__Host-ok=1; Secure; Path=/; Max-Age=60'], true, 0)
  assert.equal(jar.header(1), '__Host-ok=1')
  jar.store([`big=${'x'.repeat(5000)}; Max-Age=60`], true, 0)
  assert.equal(jar.header(1), '__Host-ok=1')
  for (let i = 0; i < 40; i++) jar.store([`c${i}=1; Max-Age=60`], true, 0)
  assert.ok((jar.header(1) as string).split('; ').length <= MAX_COOKIES)
})

test('the jar persists only durable cookies and drops expired ones on load', () => {
  const path = join(tmp(), 'jar.json')
  const jar = new Jar(path, null, 100)
  jar.store(['keep=1; Max-Age=1000', 'gone=1; Max-Age=10', 'session=1'], true, 100)
  assert.equal(statSync(path).mode & 0o777, 0o600)
  assert.equal(new Jar(path, null, 200).header(201), 'keep=1')
  assert.equal(new Jar('/nonexistent/dir/jar.json', null, 0).header(0), null)
})

class Memory implements Vault {
  items = new Map<string, Buffer>()
  broken = false
  load(account: string) { return this.items.get(account) ?? null }
  save(account: string, secret: Buffer) {
    if (this.broken) return false
    this.items.set(account, secret)
    return true
  }
  remove(account: string) { this.items.delete(account) }
}

test('the session rests in the keychain and an old file moves into it', () => {
  const path = join(tmp(), 'abc.json')
  // An earlier run left the cookie in the file.
  writeFileSync(path, '[{"name":"s","value":"v","expires":1000}]')
  const vault = new Memory()
  const jar = new Jar(path, { vault, account: 'abc' }, 100)
  assert.equal(jar.header(101), 's=v')
  assert.equal(existsSync(path), false, 'the plaintext file is gone after the move')
  assert.ok(vault.load('abc'))
  // New cookies go to the keychain only.
  jar.store(['t=w; Max-Age=500'], true, 100)
  assert.equal(existsSync(path), false)
  const again = new Jar(path, { vault, account: 'abc' }, 200)
  assert.equal(again.header(201), 's=v; t=w')
  // Logging out empties it.
  again.store(['s=; Max-Age=0', 't=; Max-Age=0'], true, 200)
  assert.equal(vault.load('abc'), null)
  assert.equal(existsSync(path), false)
})

test('a keychain that refuses leaves the private file as the fallback', () => {
  const path = join(tmp(), 'abc.json')
  const vault = new Memory()
  vault.broken = true
  const jar = new Jar(path, { vault, account: 'abc' }, 100)
  jar.store(['s=v; Max-Age=500'], true, 100)
  assert.ok(existsSync(path))
  assert.equal(statSync(path).mode & 0o777, 0o600)
  assert.equal(new Jar(path, { vault, account: 'abc' }, 200).header(201), 's=v')
})

async function instanceFor(server: TestServer, jar = new Jar(null, null, 0), limits = new Limits()): Promise<Instance> {
  return { origin: server.origin, jar, limits, now: () => Math.floor(Date.now() / 1000) }
}

async function call(handler: Parameters<typeof serve>[0], method: string, path: string, headers: [string, string][] = [], body: Uint8Array = new Uint8Array(), jar = new Jar(null, null, 0)) {
  const server = await serve(handler)
  servers.push(server)
  const instance = await instanceFor(server, jar)
  const resp = await forward(instance, method, path, headers, body)
  return { resp, request: server.requests[0], instance, server }
}

test('requests carry the instance origin and cookie and nothing the page chose', async () => {
  const jar = new Jar(null, null, 0)
  jar.store(['session=s3cret; Max-Age=3600'], false, Math.floor(Date.now() / 1000))
  const { resp, request, instance } = await call(reply(200, { 'content-type': 'application/json', 'set-cookie': 'session=new; Max-Age=3600', 'x-secret-internal': '1', server: 'x' }, '{"ok":true}'),
    'POST', '/api/messages?x=1', [
      ['Content-Type', 'application/json'], ['Origin', 'https://evil.example'], ['Cookie', 'session=stolen'], ['Referer', 'https://evil.example/'],
      ['Host', 'evil.example'], ['X-Request-Id', 'req-1'], ['Authorization', 'Bearer x'],
    ], Buffer.from('{"a":1}'), jar)
  assert.ok(request)
  assert.equal(`${request.method} ${request.url}`, 'POST /api/messages?x=1')
  assert.equal(request.headers.origin, instance.origin.origin)
  assert.equal(request.headers.cookie, 'session=s3cret')
  assert.equal(request.headers.referer, undefined)
  assert.equal(request.headers.authorization, undefined)
  assert.notEqual(request.headers.host, 'evil.example')
  assert.equal(request.headers['x-request-id'], 'req-1')
  assert.equal(request.headers['content-type'], 'application/json')
  assert.equal(request.body.toString(), '{"a":1}')
  assert.equal(resp.status, 200)
  assert.equal(Buffer.from(resp.body).toString(), '{"ok":true}')
  assert.equal(header(resp, 'content-type'), 'application/json')
  assert.equal(header(resp, 'set-cookie'), undefined)
  assert.equal(header(resp, 'x-secret-internal'), undefined)
  assert.equal(header(resp, 'server'), undefined)
  assert.equal(header(resp, 'content-security-policy'), "sandbox; default-src 'none'")
  assert.equal(header(resp, 'x-content-type-options'), 'nosniff')
  // The cookie the server set went to the jar, not to the page.
  assert.equal(instance.jar.header(instance.now() + 1), 'session=new')
})

test('an answer that could run as a document becomes a download', async () => {
  let { resp } = await call(reply(200, { 'content-type': 'text/html; charset=utf-8' }, '<script>alert(1)</script>'), 'GET', '/api/media/1')
  assert.equal(header(resp, 'content-type'), 'application/octet-stream')
  assert.equal(header(resp, 'content-disposition'), 'attachment');
  ({ resp } = await call(reply(200, { 'content-type': 'image/svg+xml', 'content-disposition': 'attachment; filename="a.svg"' }, '<svg/>'), 'GET', '/api/media/2'))
  assert.equal(header(resp, 'content-type'), 'application/octet-stream')
  assert.equal(header(resp, 'content-disposition'), 'attachment; filename="a.svg"');
  ({ resp } = await call(reply(206, { 'content-type': 'video/mp4', 'content-range': 'bytes 0-3/10', 'accept-ranges': 'bytes' }, 'abcd'), 'GET', '/api/media/3', [['Range', 'bytes=0-3']]))
  assert.deepEqual([resp.status, header(resp, 'content-type'), header(resp, 'content-range')], [206, 'video/mp4', 'bytes 0-3/10'])
  assert.equal(header(resp, 'content-disposition'), undefined)
})

test('redirects (every 3xx, as in the Rust host) and unreachable servers become gateway errors', async () => {
  const { resp } = await call(reply(302, { location: 'https://evil.example/' }, ''), 'GET', '/api/x')
  assert.equal(resp.status, 502)
  assert.equal(header(resp, 'location'), undefined)
  assert.equal((await call(reply(304, { etag: '"a"' }, ''), 'GET', '/api/x', [['If-None-Match', '"a"']])).resp.status, 502)
  const closed = await serve(reply(200, {}, ''))
  const origin = closed.origin
  await closed.close()
  const instance: Instance = { origin, jar: new Jar(null, null, 0), limits: new Limits(), now: () => 0 }
  const gone = await forward(instance, 'GET', '/api/x', [], new Uint8Array())
  assert.equal(gone.status, 502)
  assert.match(Buffer.from(gone.body).toString(), /UNAVAILABLE/)
  for (const [method, path, status] of [['GET', '/index.html', 400], ['GET', '/api/../x', 400], ['TRACE', '/api/x', 405], ['OPTIONS', '/api/x', 405]] as const) {
    assert.equal((await forward(instance, method, path, [], new Uint8Array())).status, status, `${method} ${path}`)
  }
  assert.equal((await forward(instance, 'POST', '/api/x', [], new Uint8Array(MAX_BODY + 1))).status, 413)
})

test('an answer that declares more than the cap is refused before it is read', async () => {
  const { resp } = await call((_r, res) => {
    res.writeHead(200, { 'content-length': String(MAX_BODY + 1), 'content-type': 'application/octet-stream' })
    res.write('xx')
    setTimeout(() => res.destroy(), 20)
  }, 'GET', '/api/media/x')
  assert.equal(resp.status, 502)
  assert.match(Buffer.from(resp.body).toString(), /too large for this client/)
})

test('head requests return no body', async () => {
  const { resp } = await call(reply(200, { 'content-type': 'application/json' }, ''), 'HEAD', '/api/health')
  assert.equal(resp.status, 200)
  assert.equal(resp.body.length, 0)
})

const UPLOAD = '/api/channels/00000000-0000-4000-8000-000000000000/upload'
const OVERHEAD = 1 << 20

test('request caps follow the announced upload limit', () => {
  const limits = new Limits()
  assert.equal(limits.requestCap(UPLOAD), MAX_BODY)
  assert.equal(limits.responseCap(), MAX_BODY)
  limits.announceUploadMb(10)
  assert.equal(limits.requestCap(UPLOAD), (10 << 20) + OVERHEAD)
  assert.equal(limits.requestCap('/api/channels/x/messages'), 2 << 20)
  assert.equal(limits.requestCap('/api/users/me/avatar'), (5 << 20) + OVERHEAD)
  assert.equal(limits.requestCap('/api/channels//upload'), 2 << 20)
  assert.equal(limits.requestCap('/api/channels/a/b/upload'), 2 << 20)
  // A lowered limit does not make existing files undownloadable.
  assert.equal(limits.responseCap(), MAX_BODY)
  limits.announceUploadMb(512)
  assert.equal(limits.requestCap(UPLOAD), (512 << 20) + OVERHEAD)
  assert.equal(limits.responseCap(), (512 << 20) + OVERHEAD)
  // Nonsense is ignored; the memory budget bounds even a 2 GiB limit.
  limits.announceUploadMb(0)
  limits.announceUploadMb(999_999)
  assert.equal(limits.requestCap(UPLOAD), (512 << 20) + OVERHEAD)
  limits.announceUploadMb(2048)
  assert.equal(limits.responseCap(), LARGE_BODY_BUDGET)
})

test('large bodies borrow memory and wait for each other', async () => {
  const limits = new Limits(50)
  assert.ok(await limits.lend(SMALL_BODY))
  const first = await limits.lend(LARGE_BODY_BUDGET)
  assert.ok(first)
  // The budget is spent: the next large body gives up after the wait.
  assert.equal(await limits.lend(SMALL_BODY + 1), null)
  first()
  const next = await limits.lend(SMALL_BODY + 1)
  assert.ok(next)
  // A waiting body gets the memory as soon as it is returned.
  const waiting = new Limits(5_000)
  const all = await waiting.lend(LARGE_BODY_BUDGET)
  assert.ok(all)
  const pending = waiting.lend(SMALL_BODY + 1)
  setTimeout(() => all(), 10)
  assert.ok(await pending)
})

test('an upload above the in-memory size goes through up to the announced limit', async () => {
  const permissions = await serve(reply(200, { 'content-type': 'application/json' }, '{"owner":false,"max_upload_mb":40}'))
  servers.push(permissions)
  const instance = await instanceFor(permissions)
  assert.equal((await forward(instance, 'GET', '/api/permissions', [], new Uint8Array())).status, 200)
  assert.equal(instance.limits.requestCap(UPLOAD), (40 << 20) + OVERHEAD)

  // 33 MiB is above the in-memory size and below the 40 MB limit.
  const body = Buffer.alloc(SMALL_BODY + (1 << 20), 7)
  const sink = await serve(reply(201, { 'content-type': 'application/json' }, '{}'))
  servers.push(sink)
  const upload = { ...instance, origin: sink.origin }
  const resp = await forward(upload, 'POST', UPLOAD, [['content-type', 'multipart/form-data; boundary=x']], body)
  assert.equal(resp.status, 201)
  assert.equal(sink.requests[0]?.body.length, body.length)

  // Above the limit it is refused before anything is sent, with the server's own wording.
  const refused = await forward(upload, 'POST', UPLOAD, [], new Uint8Array((41 << 20) + OVERHEAD))
  assert.equal(refused.status, 413)
  assert.match(Buffer.from(refused.body).toString(), /file exceeds the 40 MB limit/)
  assert.equal(sink.requests.length, 1)
})

test('a download above the in-memory size arrives whole and unknown lengths are held small', async () => {
  const size = SMALL_BODY + (1 << 20)
  const big = await call(reply(200, { 'content-type': 'application/octet-stream' }, Buffer.alloc(size, 1)), 'GET', '/api/media/x')
  assert.equal(big.resp.status, 200)
  assert.equal(big.resp.body.length, size)
  // Without a declared length the answer is cut at the small size.
  const chunked = await call((_r, res) => {
    res.writeHead(200, { 'content-type': 'application/octet-stream' })
    res.write(Buffer.alloc(SMALL_BODY + 1, 1))
    res.end()
  }, 'GET', '/api/media/x')
  assert.equal(chunked.resp.status, 502)
  assert.match(Buffer.from(chunked.resp.body).toString(), /cut off or too large/)
})

test('an unreadable secret store entry is kept, not treated as empty', () => {
  const path = join(tmp(), 'abc.json')
  const vault = new Memory()
  vault.items.set('abc', Buffer.from('[{"name":"s","value":"v","expires":1000}]'))
  assert.equal(new Jar(path, { vault, account: 'abc' }, 100).header(101), 's=v')
  // The keyring is locked: the entry exists but cannot be decrypted.
  const locked: Vault = { load() { throw new Error('keyring locked') }, save() { return true }, remove() { vault.items.delete('abc') } }
  assert.equal(new Jar(path, { vault: locked, account: 'abc' }, 100).header(101), null)
  assert.ok(vault.items.has('abc'), 'the stored session survives a locked keyring')
  assert.equal(new Jar(path, { vault, account: 'abc' }, 200).header(201), 's=v')
})

test('a huge Max-Age is clamped instead of becoming a cookie lost at restart', () => {
  const path = join(tmp(), 'jar.json')
  const jar = new Jar(path, null, 100)
  jar.store([`s=v; Max-Age=${'9'.repeat(400)}`], true, 100)
  assert.equal(new Jar(path, null, 200).header(201), 's=v')
})

test('a refused body reads in whole megabytes, also at the 2 GiB maximum', async () => {
  const limits = new Limits()
  limits.announceUploadMb(2048)
  const upload = '/api/channels/00000000-0000-4000-8000-000000000000/upload'
  assert.equal(limits.requestCap(upload), 2048 * 2 ** 20 + (1 << 20))
  assert.match(Buffer.from(payloadTooLarge(limits.requestCap(upload)).body).toString(), /exceeds the 2048 MB limit/)
  // Above what the memory budget can lend, a retry cannot succeed: 413, not 503.
  assert.deepEqual(limits.bodyLimit(upload), { cap: LARGE_BODY_BUDGET, file: true })
  assert.match(Buffer.from(payloadTooLarge(LARGE_BODY_BUDGET).body).toString(), /exceeds the 1023 MB limit/)
  assert.deepEqual(limits.bodyLimit('/api/channels/x/messages'), { cap: 2 << 20, file: false })
  assert.doesNotMatch(Buffer.from(payloadTooLarge(2 << 20, false).body).toString(), /file/)
})

test('a waiter that gives up lets the smaller ones behind it through', async () => {
  const unit = 64 << 10
  const small = new Limits(50, 2 * (SMALL_BODY + unit))
  const first = await small.lend(SMALL_BODY + 1)
  assert.ok(first)
  // The head asks for more than is free and gives up soon; the one behind it fits.
  const head = small.lend(SMALL_BODY + unit * 20)
  const tail = small.lend(SMALL_BODY + 1)
  assert.equal(await head, null)
  assert.ok(await tail, 'the tail is granted when the head times out, not only when the first transfer ends')
})
