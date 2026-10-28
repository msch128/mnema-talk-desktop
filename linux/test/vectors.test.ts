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
import { appPath, externalAllowed, inviteCode, parseAddress, parseHealth } from '../src/core/client.ts'
import { Jar } from '../src/core/cookies.ts'
import { verifyPackage } from '../src/core/minisign.ts'
import { Limits, clampRange, inlineSafe, mimeFor, serveBundle, target } from '../src/core/proxy.ts'
import { Store } from '../src/core/store.ts'
import { type Statement, TrustState, chooseUpdaterKey, minisignKeyId, parseUtc, rootKey, signatureKeyId, statementBundleKeys, statementCaps, verifyStatement } from '../src/core/trust.ts'
import { formatVersion, parseVersion } from '../src/core/version.ts'
import { sha256 } from './helpers/fixtures.ts'

type Json = any // eslint-disable-line @typescript-eslint/no-explicit-any -- vector files are untyped JSON
const load = (name: string): Json => JSON.parse(readFileSync(new URL(`../../test-vectors/${name}.json`, import.meta.url), 'utf8'))
const keys = (hexes: string[]): TrustedKey[] => hexes.map((h) => parseKey(h) as TrustedKey)

test('bundle vectors: versions, paths and verification', () => {
  const v = load('bundle')
  for (const c of v.versions) assert.deepEqual(parseVersion(c.input), c.parsed, JSON.stringify(c.input))
  for (const c of v.paths) assert.equal(validPath(c.path), c.valid, JSON.stringify(c.path))
  for (const c of v.bundles) {
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
