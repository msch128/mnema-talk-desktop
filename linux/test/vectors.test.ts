/**
 * The shared vectors (`test-vectors/*.json`). The Rust host reads the same
 * files (`experimental/app/src/*.rs`, test `shared_vectors`). Every case must
 * give the answer written in the file.
 */
import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, rmSync, unlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { type TrustedKey, parseKey, validPath, verifyBundle } from '../src/core/bundle.ts'
import { SELECTOR_ONLY, appPath, externalAllowed, inviteCode, parseAddress, parseHealth } from '../src/core/client.ts'
import { Jar } from '../src/core/cookies.ts'
import { verifyPackage } from '../src/core/minisign.ts'
import { Limits, clampRange, inlineSafe, mimeFor, serveBundle, target } from '../src/core/proxy.ts'
import { INVALID_RESPONSE, NAT_HINT, UNAVAILABLE, unreachableReason } from '../src/core/discover.ts'
import { AddError, MAX_INSTANCES } from '../src/core/instances.ts'
import { Store, refusalText } from '../src/core/store.ts'
import { WITHDRAWN } from '../src/core/trusted.ts'
import { bundleAsset } from '../src/core/updates.ts'
import { type Statement, TrustState, chooseUpdaterKey, minisignKeyId, parseUtc, rootKey, signatureKeyId, statementBundleKeys, statementCaps, verifyStatement } from '../src/core/trust.ts'
import { formatVersion, parseVersion } from '../src/core/version.ts'
import { sha256 } from './helpers/fixtures.ts'

type Json = any // eslint-disable-line @typescript-eslint/no-explicit-any -- vector files are untyped JSON
const load = (name: string): Json => JSON.parse(readFileSync(new URL(`../../test-vectors/${name}.json`, import.meta.url), 'utf8'))
const keys = (hexes: string[]): TrustedKey[] => hexes.map((h) => parseKey(h) as TrustedKey)

function checkBundles(cases: Json[]): void {
  for (const c of cases) {
    let result: Json
    try {
      const b = verifyBundle(Buffer.from(c.zip, 'base64'), keys(c.trusted))
      const files: Json = {}
      for (const path of Object.keys(c.result.ok?.files ?? {})) files[path] = sha256(b.file(path) as Uint8Array)
      result = { ok: { version: formatVersion(b.version), web_client_api: b.webClientApi, min_server_version: formatVersion(b.minServerVersion), security_floor: formatVersion(b.securityFloor), key_id: b.keyId, commit: b.commit, files } }
    } catch (e) {
      result = { error: (e as Error).message }
    }
    assert.deepEqual(result, c.result, c.name)
  }
}

test('bundle vectors: versions, paths and verification', () => {
  const v = load('bundle')
  for (const c of v.versions) assert.deepEqual(parseVersion(c.input), c.parsed, JSON.stringify(c.input))
  for (const c of v.paths) assert.equal(validPath(c.path), c.valid, JSON.stringify(c.path))
  checkBundles(v.bundles)
})

test('zip vectors: structures that both clients must refuse alike', () => {
  const v = load('zip')
  checkBundles(v.bundles)
  const open = v.bundles.filter((c: Json) => c.result.ok).map((c: Json) => c.name)
  assert.deepEqual(open, ['a plain stored bundle', 'an entry that announces a data descriptor', 'an entry with a regular file mode', 'a link mode written by a system that does not use modes'])
})

test('json vectors: byte order marks, repeated keys, unknown fields, wrong types and trailing data', () => {
  const v = load('json')
  checkBundles(v.bundles)
  const root = rootKey(v.root)
  for (const c of v.statements) {
    let verify: Json
    try {
      const st = verifyStatement(Buffer.from(c.doc, 'base64'), Buffer.from(c.sig), root)
      verify = { ok: { serial: st.serial, bundle_keys: statementBundleKeys(st).map((k) => k.id), caps: Object.fromEntries([...statementCaps(st)].map(([k, n]) => [String(k), formatVersion(n)])) } }
    } catch (e) {
      verify = { error: (e as Error).message }
    }
    assert.deepEqual(verify, c.verify, c.name)
  }
  for (const c of v.healths) {
    let result: Json
    try {
      const info = parseHealth(Buffer.from(c.input, 'base64'))
      result = { ok: { version: info.version === null ? null : formatVersion(info.version), api: info.webClientApi } }
    } catch (e) {
      result = { error: (e as Error).message }
    }
    assert.deepEqual(result, c.result, c.name)
  }
  for (const c of v.releases) {
    const found = bundleAsset(Buffer.from(c.input, 'base64'))
    assert.deepEqual(found === null ? null : { version: formatVersion(found.version), url: found.url.href }, c.result, c.name)
  }
  // Each list opens its plain documents and refuses everything else listed as a fault.
  const opened = (list: Json[], ok: (c: Json) => boolean) => list.filter(ok).map((c) => c.name)
  assert.deepEqual(opened(v.statements, (c) => c.verify.ok), ['a plain document', 'spaces and newlines around the document', 'an escaped key'])
  assert.deepEqual(opened(v.healths, (c) => c.result.ok), ['a plain document', 'spaces and newlines around the document', 'an unknown field', 'an escaped value', 'an unknown field nested 100 deep', 'an unknown field nested 120 deep', 'an unknown field nested 125 deep', 'an unknown field nested 126 deep'])
})

test('store vectors: selection, rollback marks, revocation and caps', () => {
  const v = load('store')
  const pick = (names: string[]) => keys(names.map((n) => v.keys[n]))
  const bytes = (name: string) => Buffer.from(v.bundles[name], 'base64')
  for (const s of v.scenarios) {
    const dir = s.persist ? mkdtempSync(join(tmpdir(), 'mnema-vec-')) : null
    try {
      const builtin = s.builtin ? bytes(s.builtin) : null
      let store = Store.load(dir, builtin, pick(s.trusted))
      let n = 0
      for (const step of s.steps) {
        const where = `${s.name} #${n++} ${step.op}`
        switch (step.op) {
          case 'install': {
            let got: Json
            try { got = { ok: formatVersion(store.install(bytes(step.bundle), pick(step.trusted))) } } catch (e) { got = { error: (e as Error).message } }
            assert.deepEqual(got, step.expect, where)
            break
          }
          case 'select': {
            const r = store.select({ version: step.server.version === null ? null : parseVersion(step.server.version), webClientApi: step.server.api })
            const got = r.ok ? { ok: formatVersion(r.held.bundle.version) } : { refusal: r.refusal.kind, ...(r.refusal.kind === 'server_too_old' ? { needs: formatVersion(r.refusal.needs) } : {}) }
            assert.deepEqual(got, step.expect, where)
            break
          }
          case 'reconcile': assert.equal(store.reconcile(pick(step.trusted)), step.expect, where); break
          case 'caps': store.applyCaps(new Map(Object.entries(step.caps as Record<string, string>).map(([k, ver]) => [Number(k), parseVersion(ver) as [number, number, number]]))); break
          case 'state': {
            const highWater: Json = {}
            for (const api of [1, 2, 3]) { const m = store.highWater(api); if (m) highWater[String(api)] = formatVersion(m) }
            assert.deepEqual({ floor: formatVersion(store.floor()), high_water: highWater, held: store.held().map((h) => `${formatVersion(h.bundle.version)}:${h.source}`), rejected: store.rejected.length }, step.expect, where)
            break
          }
          case 'restart': store = Store.load(dir, builtin, pick(step.trusted)); break
          case 'remove_cached': unlinkSync(join(dir as string, `mnema-web-${step.version}.zip`)); break
          case 'forget_state': if (existsSync(join(dir as string, 'state.json'))) unlinkSync(join(dir as string, 'state.json')); break
          default: assert.fail(`unknown step ${step.op}`)
        }
      }
    } finally {
      if (dir) rmSync(dir, { recursive: true, force: true })
    }
  }
})

test('trust vectors: times, statements, serial order, minisign ids and updater key choice', () => {
  const v = load('trust')
  const root = rootKey(v.root)
  for (const c of v.times) assert.equal(parseUtc(c.input), c.secs, JSON.stringify(c.input))
  for (const c of v.statements) {
    let verify: Json
    try {
      const st = verifyStatement(Buffer.from(c.doc), Buffer.from(c.sig), root)
      verify = { ok: { serial: st.serial, bundle_keys: statementBundleKeys(st).map((k) => k.id), caps: Object.fromEntries([...statementCaps(st)].map(([k, ver]) => [String(k), formatVersion(ver)])), updater_keys: st.updater_keys.length } }
    } catch (e) { verify = { error: (e as Error).message } }
    assert.deepEqual(verify, c.verify, `verify: ${c.name}`)
    let accept: Json
    try { accept = TrustState.load(null, root).accept(Buffer.from(c.doc), Buffer.from(c.sig), c.now) } catch (e) { accept = { error: (e as Error).message } }
    assert.deepEqual(accept, c.accept, `accept: ${c.name}`)
  }
  for (const seq of v.sequences) {
    const trust = TrustState.load(null, root)
    for (const [i, s] of seq.steps.entries()) {
      let got: Json
      try { got = trust.accept(Buffer.from(s.doc), Buffer.from(s.sig), s.now) } catch (e) { got = { error: (e as Error).message } }
      assert.deepEqual(got, s.expect, `${seq.name} #${i}`)
      assert.equal(trust.statement()?.serial ?? null, s.serial, `${seq.name} #${i} serial`)
    }
  }
  for (const c of v.minisign_ids) assert.equal(c.kind === 'key' ? minisignKeyId(c.input) : signatureKeyId(c.input), c.id, `${c.kind} ${c.input.slice(0, 20)}`)
  for (const c of v.updater_choices) {
    const statement: Statement | null = c.statement && { schema: 1, serial: 2, issued_at: 'x', expires_at: 'y', bundle_keys: [], revoked_bundle_keys: [], max_version: {}, ...c.statement }
    const r = chooseUpdaterKey(c.pinned, statement, c.signature)
    assert.deepEqual(r.ok ? { key: r.key } : { refusal: r.refusal }, c.expect, c.name)
  }
})

test('proxy vectors: media types, targets, caps, mime types, served files and the cookie jar', () => {
  const v = load('proxy')
  for (const c of v.inline_safe) assert.equal(inlineSafe(c.input), c.safe, JSON.stringify(c.input))
  for (const c of v.targets) assert.equal(target(new URL(c.origin), c.input)?.href ?? null, c.href, JSON.stringify(c.input))
  for (const c of v.caps) {
    const limits = new Limits()
    for (const mb of c.announce) limits.announceUploadMb(mb)
    assert.deepEqual([limits.requestCap(c.path), limits.responseCap()], [c.request_cap, c.response_cap], JSON.stringify(c))
  }
  for (const c of v.ranges) assert.equal(clampRange(c.input), c.clamped, JSON.stringify(c.input))
  for (const c of v.mimes) assert.equal(mimeFor(c.name), c.type, c.name)
  const bundle = verifyBundle(Buffer.from(v.serve.zip, 'base64'), keys(v.serve.trusted))
  for (const c of v.serve.cases) {
    const r = serveBundle(bundle, c.method, c.path)
    const h = (n: string) => r.headers.find(([k]) => k === n)?.[1] ?? null
    assert.deepEqual({ status: r.status, content_type: h('content-type'), cache_control: h('cache-control'), allow: h('allow'), body: Buffer.from(r.body).toString() },
      { status: c.status, content_type: c.content_type, cache_control: c.cache_control, allow: c.allow, body: c.body }, `${c.method} ${c.path}`)
  }
  for (const j of v.jars) {
    const jar = new Jar(null, null, 0)
    for (const [i, s] of j.steps.entries()) {
      if (s.op === 'store') jar.store(s.set_cookie, s.https, s.now)
      else assert.equal(jar.header(s.now), s.expect, `${j.name} #${i}`)
    }
  }
})

test('client vectors: addresses, health, external links and app routes', () => {
  const v = load('client')
  for (const c of v.addresses) {
    let got: Json
    try { got = { input: c.input, href: parseAddress(c.input).href } } catch (e) { got = { input: c.input, error: (e as Error).message } }
    assert.deepEqual(got, c, JSON.stringify(c.input))
  }
  for (const c of v.healths) {
    let got: Json
    try {
      const info = parseHealth(Buffer.from(c.input))
      got = { input: c.input, ok: { version: info.version === null ? null : formatVersion(info.version), api: info.webClientApi } }
    } catch (e) { got = { input: c.input, error: (e as Error).message } }
    assert.deepEqual(got, c, c.input)
  }
  for (const c of v.externals) assert.equal(externalAllowed(new URL(c.url)), c.allowed, c.url.slice(0, 60))
  for (const c of v.app_paths) assert.equal(appPath(c.path), c.ok, JSON.stringify(c.path))
  for (const c of v.invites) assert.equal(inviteCode(c.input), c.code, c.input)
})

test('minisign vectors', () => {
  const v = load('minisign')
  for (const c of v.cases) assert.equal(verifyPackage(Buffer.from(c.package), c.signature, c.pubkey, c.version), c.expect, c.name)
})

/**
 * The connect page translates the sentences this client sends by exact text.
 * They are the ones written in `test-vectors/host-errors.json`, also read by
 * the Rust host and copied into the web app.
 */
test('the sentences the connect page can receive are exactly the listed ones', () => {
  const list = load('host-errors')
  const render = (text: string): string => text.replace('{version}', '0.8.9').replace('{count}', String(MAX_INSTANCES))
  const error = (code: string): Error => Object.assign(new Error('x'), { code })
  const message = (run: () => unknown): string => {
    try { run() } catch (e) { return (e as Error).message }
    throw new Error('expected an error')
  }
  const sent: Record<string, string> = {
    enterAddress: message(() => parseAddress('a\u0001b')),
    invalidAddress: message(() => parseAddress('a b')),
    httpsOrigin: message(() => parseAddress('https://user@example.org')),
    refused: unreachableReason(error('ECONNREFUSED')),
    timeout: unreachableReason(error('ETIMEDOUT')),
    dns: unreachableReason(error('ENOTFOUND')),
    certificate: unreachableReason(error('CERT_HAS_EXPIRED')),
    unreachable: unreachableReason(new Error('x')),
    unavailable: UNAVAILABLE,
    invalidResponse: INVALID_RESPONSE,
    instanceNeedsUpdate: message(() => parseHealth(Buffer.from('{}'))),
    notCompatible: message(() => parseHealth(Buffer.from('{"status":"down"}'))),
    noBundle: refusalText({ kind: 'no_bundle' }),
    serverNewer: refusalText({ kind: 'server_newer' }),
    serverTooOld: refusalText({ kind: 'server_too_old', needs: parseVersion('0.8.9') as never }),
    contractRetired: refusalText({ kind: 'server_contract_retired' }),
    signerWithdrawn: refusalText({ kind: 'signer_withdrawn' }),
    rolledBack: refusalText({ kind: 'rolled_back' }),
    alreadyListed: new AddError('known').message,
    maxServers: new AddError('full').message,
    withdrawn: WITHDRAWN,
    selectorOnly: SELECTOR_ONLY,
  }
  assert.equal(list.natHint, NAT_HINT)
  const mine = list.sentences.filter((s: Json) => s.clients.includes('linux'))
  assert.deepEqual(mine.map((s: Json) => s.id).sort(), Object.keys(sent).sort())
  for (const s of mine) {
    const expected = render(s.text)
    const got = sent[s.id] as string
    if (s.withHint) assert.equal(got, `${expected} ${list.natHint}`, s.id)
    else if (s.withDetail) assert.ok(got === expected || got.startsWith(`${expected} (`), s.id)
    else assert.equal(got, expected, s.id)
  }
})
