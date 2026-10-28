/**
 * Writes the shared test vectors (`test-vectors/*.json`). The Rust host and the
 * TypeScript client read the same files and must agree on every case: bundle
 * verification, rollback marks, trust statements, the proxy rules, the address
 * and health rules, and minisign verification.
 *
 * The cases are built from fixed one-byte key seeds, so the output is
 * deterministic. `--check` fails if the committed files differ.
 *
 * The expectations are what the Rust host answers (this file was written
 * against its tests); a mismatch on either side is a bug in that side.
 */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { MAX_FILE, MAX_PATH, parseKey, validPath, verifyBundle } from '../src/core/bundle.ts'
import { appPath, externalAllowed, parseAddress, parseHealth } from '../src/core/client.ts'
import { Jar } from '../src/core/cookies.ts'
import { Limits, inlineSafe, mimeFor, serveBundle, target } from '../src/core/proxy.ts'
import { Store } from '../src/core/store.ts'
import { chooseUpdaterKey, minisignKeyId, parseUtc, signatureKeyId, TrustState, rootKey, statementBundleKeys, statementCaps, verifyStatement } from '../src/core/trust.ts'
import { verifyPackage } from '../src/core/minisign.ts'
import { formatVersion, parseVersion } from '../src/core/version.ts'
import {
  bundle, entries, entry, minisignKey, minisignSignature, resigned, sha256, signText, spec, statementText, testKey, trusted, zipEntries, zipOf,
  type Entry, type Spec, type StatementSpec,
} from '../test/helpers/fixtures.ts'

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'test-vectors')
const b64 = (bytes: Uint8Array) => Buffer.from(bytes).toString('base64')
const hex = (seed: number) => testKey(seed).hex
const keysOf = (...seeds: number[]) => seeds.map(hex)

type Json = Record<string, unknown>

// ------------------------------------------------------------------ bundle

function bundleVectors(): Json {
  const versions = ['0.7.16', '0.0.0', '1.2.3', '123456789.0.0', '', '1', '1.2', '1.2.3.4', 'v1.2.3', '01.2.3', '1.2.x', '1..3', '1.2.3-rc1', '-1.0.0', '99999999999.0.0', '+1.0.0', '1.2.3 ', ' 1.2.3']
    .map((input) => ({ input, parsed: parseVersion(input) }))
  const paths = ['index.html', 'assets/a-b.js', '\u00fcn\u00ef/x.js', 'a b/c', '', '/abs', 'a//b', 'a/./b', 'a/../b', '..', '.', 'a/', 'a\\b', 'a\0b', 'a\u007f', 'a\nb', 'a'.repeat(MAX_PATH), 'a'.repeat(MAX_PATH + 1)]
    .map((path) => ({ path, valid: validPath(path) }))

  const base = spec('0.8.0')
  const good = entries(base)
  const edit = (change: (list: Entry[]) => void): Buffer => {
    const list = entries(base)
    change(list)
    return zipEntries(list)
  }
  const cases: { name: string, trusted: string[], zip: Buffer }[] = [
    { name: 'a bundle signed by a trusted key', trusted: keysOf(7), zip: bundle(base) },
    { name: 'a deflated bundle', trusted: keysOf(7), zip: zipOf(good.map((e) => ({ name: e.name, body: e.body, method: 8 as const }))) },
    { name: 'a bundle with a second trusted key listed first', trusted: keysOf(5, 7), zip: bundle(base) },
    { name: 'another key', trusted: keysOf(8), zip: bundle(base) },
    { name: 'no keys', trusted: [], zip: bundle(base) },
    { name: 'not a zip', trusted: keysOf(7), zip: Buffer.from('PK not a zip') },
    { name: 'a changed file', trusted: keysOf(7), zip: edit((e) => { entry(e, 'app/index.html').body = Buffer.from('<html>evil</html>'.slice(0, 16)) }) },
    { name: 'a changed file of another size', trusted: keysOf(7), zip: edit((e) => { entry(e, 'app/index.html').body = Buffer.from('<html>evil</html>') }) },
    { name: 'an extra file', trusted: keysOf(7), zip: edit((e) => { e.push({ name: 'app/evil.js', body: Buffer.from('x') }) }) },
    { name: 'a missing file', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1) }) },
    {
      name: 'a file swapped for another name', trusted: keysOf(7), zip: edit((e) => {
        e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1)
        e.push({ name: 'app/assets/other.js', body: Buffer.from('console.log(1)') })
      }),
    },
    { name: 'no manifest', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.json'), 1) }) },
    { name: 'no signature', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.sig'), 1) }) },
    { name: 'an edited manifest', trusted: keysOf(7), zip: edit((e) => { const m = entry(e, 'manifest.json'); m.body = Buffer.from(m.body.toString().replace('0.8.0', '9.9.9')) }) },
    { name: 'a signature that is not base64', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.from('***') }) },
    { name: 'a signature that is too short', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.from('AAAA') }) },
    { name: 'a signature with whitespace around it', trusted: keysOf(7), zip: edit((e) => { const s = entry(e, 'manifest.sig'); s.body = Buffer.from(`\n  ${s.body.toString()}\r\n`) }) },
    { name: 'a signature above the limit', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.alloc(2000, 0x41) }) },
  ]
  const files = (m: Record<string, unknown>) => m.files as Record<string, unknown>[]
  const resignedCases: [string, (m: Record<string, unknown>) => void, Spec?][] = [
    ['an unchanged manifest, signed again', () => {}],
    ['schema 2', (m) => { m.schema = 2 }],
    ['another name', (m) => { m.name = 'other' }],
    ['a key id that is not the signer', (m) => { m.key_id = '0000000000000000' }],
    ['a version that is not a version', (m) => { m.version = '1' }],
    ['web_client_api 0', (m) => { m.web_client_api = 0 }],
    ['a minimum server newer than the bundle', (m) => { m.min_server_version = '9.0.0' }],
    ['a floor above the bundle', (m) => { m.security_floor = '9.0.0' }],
    ['a minimum server that is not a version', (m) => { m.min_server_version = 'x' }],
    ['a floor that is not a version', (m) => { m.security_floor = '1.2' }],
    ['no files', (m) => { m.files = [] }],
    ['an unknown field', (m) => { m.surprise = 1 }],
    ['an unknown field of a file', (m) => { files(m)[0] = { ...files(m)[0], extra: 1 } }],
    ['a missing field', (m) => { delete m.commit }],
    ['a path with dot dot', (m) => { (files(m)[0] as Record<string, unknown>).path = '../x' }],
    ['a duplicated path', (m) => { (files(m)[1] as Record<string, unknown>).path = (files(m)[0] as Record<string, unknown>).path }],
    ['a hash that is not hex', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'zz' }],
    ['an upper case hash', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'A'.repeat(64) }],
    ['a size above the file limit', (m) => { (files(m)[0] as Record<string, unknown>).size = MAX_FILE + 1 }],
    ['a size that is not the size', (m) => { (files(m)[0] as Record<string, unknown>).size = 1 }],
    ['a hash that is not the hash', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = '0'.repeat(64) }],
    ['no app shell', (m) => { (files(m)[0] as Record<string, unknown>).path = 'home.html' }],
    ['a total above the limit', (m) => {
      const list = files(m)
      for (let i = 0; i < 5; i++) list.push({ path: `big${i}`, sha256: '0'.repeat(64), size: MAX_FILE })
      for (const f of list) f.size = MAX_FILE
    }],
    ['a newer bundle with a floor and a minimum server', () => {}, spec('0.9.0', { floor: '0.8.5', minServer: '0.8.9' })],
  ]
  for (const [name, change, s] of resignedCases) cases.push({ name, trusted: keysOf(7), zip: resigned(change, s) })

  const out = cases.map(({ name, trusted: keys, zip }) => {
    let result: Json
    try {
      const b = verifyBundle(zip, trusted(...keys.map((k) => [7, 5, 8].find((s) => hex(s) === k) as number)))
      const paths = ['index.html', 'assets/app-1.js', '\u00fcn\u00ef/x.js'].filter((p) => b.file(p) !== undefined)
      result = {
        ok: {
          version: formatVersion(b.version), web_client_api: b.webClientApi, min_server_version: formatVersion(b.minServerVersion),
          security_floor: formatVersion(b.securityFloor), key_id: b.keyId, commit: b.commit,
          files: Object.fromEntries(paths.map((p) => [p, sha256(b.file(p) as Uint8Array)])),
        },
      }
    } catch (error) {
      result = { error: (error as Error).message }
    }
    return { name, trusted: keys, zip: b64(zip), result }
  })
  return { versions, paths, bundles: out }
}

// ------------------------------------------------------------------- store

type Step = Json

function storeVectors(): Json {
  const named: Record<string, Spec> = {
    b080: spec('0.8.0'),
    b080old: spec('0.8.0', { minServer: '0.7.0' }),
    b081: spec('0.8.1'),
    b082fix: spec('0.8.2', { floor: '0.8.1' }),
    b083: spec('0.8.3'),
    b084: spec('0.8.4'),
    b090: spec('0.9.0', { minServer: '0.8.9' }),
    b090floor: spec('0.9.0', { floor: '0.8.5' }),
    b099: spec('0.99.0', { floor: '0.5.0' }),
    api2: spec('1.0.0', { api: 2 }),
    api2b090: spec('0.9.0', { api: 2 }),
    forged080: spec('0.8.0', { seed: 5 }),
    forged099: spec('0.99.0', { seed: 5, floor: '0.99.0' }),
    other090: spec('0.9.0', { seed: 9 }),
  }
  const bundles = Object.fromEntries(Object.entries(named).map(([k, v]) => [k, b64(bundle(v))]))
  const keys = { a: hex(7), b: hex(5), c: hex(9) }
  const srv = (version: string | null, api = 1) => ({ version, api })
  const install = (name: string, trustedKeys: string[]): Step => ({ op: 'install', bundle: name, trusted: trustedKeys })
  const select = (server: Json): Step => ({ op: 'select', server })
  const state: Step = { op: 'state' }

  const scenarios: { name: string, persist: boolean, builtin: string | null, trusted: string[], steps: Step[] }[] = [
    {
      name: 'the server is matched by contract and release', persist: false, builtin: 'b080old', trusted: ['a'],
      steps: [install('b090', ['a']), select(srv('0.9.0')), select(srv('0.8.5')), select(srv(null)), select(srv('0.6.0')), select(srv('0.9.0', 2)), state],
    },
    { name: 'nothing speaks an older contract any more', persist: false, builtin: 'api2', trusted: ['a'], steps: [select(srv(null)), state] },
    { name: 'a store with nothing in it', persist: false, builtin: null, trusted: ['a'], steps: [select(srv(null)), state] },
    {
      name: 'the high-water mark blocks older bundles and survives a restart', persist: true, builtin: 'b080', trusted: ['a'],
      steps: [
        install('b084', ['a']), state, select(srv('0.1.0')),
        { op: 'remove_cached', version: '0.8.4' }, { op: 'restart', trusted: ['a'] }, state, select(srv('0.1.0')),
        { op: 'forget_state' }, { op: 'restart', trusted: ['a'] }, state,
        install('api2b090', ['a']), state, select(srv(null)),
      ],
    },
    {
      name: 'a forged mark of a revoked key is cleared', persist: true, builtin: 'b080', trusted: ['a', 'b'],
      steps: [
        install('forged099', ['a', 'b']), state, { op: 'reconcile', trusted: ['a'], expect: true }, state, select(srv(null)),
        { op: 'reconcile', trusted: ['a'], expect: false },
      ],
    },
    {
      name: 'the cleared marks stay cleared after a restart', persist: true, builtin: 'b080', trusted: ['a', 'b'],
      steps: [install('forged099', ['a', 'b']), { op: 'restart', trusted: ['a'] }, state, select(srv(null)), { op: 'restart', trusted: ['a', 'b'] }, state],
    },
    {
      name: 'marks and floors of still trusted keys are kept', persist: true, builtin: 'b080', trusted: ['a', 'b'],
      steps: [install('b090floor', ['a', 'b']), install('forged099', ['a', 'b']), { op: 'reconcile', trusted: ['a'], expect: true }, state, select(srv(null))],
    },
    {
      name: 'when only revoked bundles are left the client says so until a new one arrives', persist: false, builtin: 'forged080', trusted: ['b'],
      steps: [select(srv(null)), { op: 'reconcile', trusted: ['a'], expect: true }, select(srv(null)), install('b081', ['a']), select(srv(null)), state],
    },
    {
      name: 'a cap lowers marks and drops bundles above it', persist: false, builtin: 'b080', trusted: ['a'],
      steps: [
        install('b099', ['a']), install('api2', ['a']), { op: 'caps', caps: {} }, state,
        { op: 'caps', caps: { 1: '0.9.0' } }, state,
      ],
    },
    {
      name: 'a cap clears the floor a forged bundle of a trusted key raised', persist: false, builtin: 'b080', trusted: ['a', 'b'],
      steps: [install('forged099', ['a', 'b']), state, { op: 'caps', caps: { 1: '0.8.0' } }, state, select(srv(null))],
    },
    {
      name: 'a cap removes the cached file of a bundle above it, so a restart does not bring it back', persist: true, builtin: 'b080', trusted: ['a'],
      steps: [install('b099', ['a']), { op: 'caps', caps: { 1: '0.9.0' } }, state, { op: 'restart', trusted: ['a'] }, state, select(srv(null))],
    },
    {
      name: 'the security floor only moves up and withdraws older bundles', persist: true, builtin: 'b080', trusted: ['a'],
      steps: [
        state, install('b082fix', ['a']), state, { op: 'remove_cached', version: '0.8.2' }, { op: 'restart', trusted: ['a'] }, state,
        install('b080', ['a']), install('b081', ['a']),
      ],
    },
    {
      name: 'updates install only newer verified bundles', persist: true, builtin: 'b080', trusted: ['a'],
      steps: [install('b080', ['a']), install('other090', ['a']), install('b081', ['a']), { op: 'restart', trusted: ['a'] }, state, install('b081', ['a'])],
    },
    {
      name: 'the cache keeps only the newest few versions', persist: true, builtin: 'b080', trusted: ['a'],
      steps: [install('b081', ['a']), install('b082fix', ['a']), install('b083', ['a']), install('b084', ['a']), state],
    },
  ]
  // Replay the scenarios on the TypeScript store to write what it answers. The Rust
  // host replays the same file; a difference is a bug on one side.
  const out = scenarios.map((s) => ({ ...s, steps: replay(s, named, keys) }))
  return { keys, bundles, scenarios: out }
}

function trustedOf(names: string[], keys: Record<string, string>) {
  return names.map((n) => parseKey(keys[n] as string) as NonNullable<ReturnType<typeof parseKey>>)
}

/** Runs the steps on the TypeScript store and adds the `expect` of each. */
function replay(s: { persist: boolean, builtin: string | null, trusted: string[], steps: Step[] }, named: Record<string, Spec>, keys: Record<string, string>): Step[] {
  const dir = s.persist ? mkdtempSync(join(tmpdir(), 'mnema-vectors-')) : null
  const builtin = s.builtin ? bundle(named[s.builtin] as Spec) : null
  let store = Store.load(dir, builtin, trustedOf(s.trusted, keys))
  const out: Step[] = []
  try {
    for (const step of s.steps) {
      switch (step.op) {
        case 'install': {
          let expect: Json
          try {
            expect = { ok: formatVersion(store.install(bundle(named[step.bundle as string] as Spec), trustedOf(step.trusted as string[], keys))) }
          } catch (error) {
            expect = { error: (error as Error).message }
          }
          out.push({ ...step, expect })
          break
        }
        case 'select': {
          const server = step.server as { version: string | null, api: number }
          const result = store.select({ version: server.version === null ? null : parseVersion(server.version), webClientApi: server.api })
          out.push({
            ...step,
            expect: result.ok ? { ok: formatVersion(result.held.bundle.version) }
              : { refusal: result.refusal.kind, ...(result.refusal.kind === 'server_too_old' ? { needs: formatVersion(result.refusal.needs) } : {}) },
          })
          break
        }
        case 'reconcile':
          out.push({ ...step, expect: store.reconcile(trustedOf(step.trusted as string[], keys)) })
          break
        case 'caps': {
          const caps = new Map(Object.entries(step.caps as Record<string, string>).map(([k, v]) => [Number(k), parseVersion(v) as NonNullable<ReturnType<typeof parseVersion>>]))
          store.applyCaps(caps)
          out.push(step)
          break
        }
        case 'state': {
          const highWater: Record<string, string> = {}
          for (const api of [1, 2, 3]) {
            const v = store.highWater(api)
            if (v) highWater[String(api)] = formatVersion(v)
          }
          out.push({ ...step, expect: { floor: formatVersion(store.floor()), high_water: highWater, held: store.held().map((h) => `${formatVersion(h.bundle.version)}:${h.source}`), rejected: store.rejected.length } })
          break
        }
        case 'restart':
          store = Store.load(dir, builtin, trustedOf(step.trusted as string[], keys))
          out.push(step)
          break
        case 'remove_cached':
          unlinkSync(join(dir as string, `mnema-web-${step.version as string}.zip`))
          out.push(step)
          break
        case 'forget_state':
          if (existsSync(join(dir as string, 'state.json'))) unlinkSync(join(dir as string, 'state.json'))
          out.push(step)
          break
        default:
          throw new Error(`unknown step ${String(step.op)}`)
      }
    }
  } finally {
    if (dir) rmSync(dir, { recursive: true, force: true })
  }
  return out
}

// ------------------------------------------------------------------- trust

const NOW = 1_793_491_200 // 2026-11-01T00:00:00Z

function trustVectors(): Json {
  const rootSeed = 9
  const rootPub = hex(rootSeed)
  const doc = (serial: number, allowed: number[], revoked: number[], extra: Partial<StatementSpec> = {}) => statementText({
    serial, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z',
    bundle_keys: allowed.map(hex), revoked_bundle_keys: revoked.map((s) => testKey(s).id), ...extra,
  })
  const signedBy = (seed: number, text: string) => ({ doc: text, sig: signText(testKey(seed), text) })
  const pair = minisignKey(3)
  const pair2 = minisignKey(4)

  const base = doc(1, [1], [])
  const statementCases: { name: string, doc: string, sig: string, now: number }[] = []
  const add = (name: string, text: string, signer = rootSeed, now = NOW) => statementCases.push({ name, ...signedBy(signer, text), now })
  add('a statement signed by the root key', doc(1, [1], []))
  add('a statement with revocations', doc(2, [2], [1]))
  add('a statement with caps', doc(1, [1], [], { max_version: { 1: '0.9.0', 2: '1.0.0' } }))
  add('a statement with updater keys', doc(1, [1], [], { updater_keys: [pair.pubkey, pair2.pubkey], revoked_updater_keys: ['0123456789abcdef'] }))
  add('signed by another key', doc(1, [1], []), 8)
  add('serial 0', doc(0, [1], []))
  add('no bundle key', doc(1, [], []))
  add('a key both allowed and revoked', doc(1, [1], [1]))
  add('an empty object', '{}')
  add('schema 2', base.replace('"schema":1', '"schema":2'))
  add('an unknown field', base.replace('"serial"', '"extra":1,"serial"'))
  add('a validity of zero', base.replace('2027-01-15', '2026-10-17'))
  add('a validity above 400 days', base.replace('2027-01-15', '2029-01-15'))
  add('a time that is not a time', base.replace('2027-01-15T10:00:00Z', 'soon'))
  add('a cap for api x', doc(1, [1], [], { max_version: { x: '0.9.0' } }))
  add('a cap for api 0', doc(1, [1], [], { max_version: { 0: '0.9.0' } }))
  add('a cap that is not a version', doc(1, [1], [], { max_version: { 1: 'latest' } }))
  add('a revoked id that is not an id', doc(1, [1], [], { revoked_bundle_keys: ['xyz'] }))
  add('a bundle key that is not a key', doc(1, [1], [], { bundle_keys: ['00'] }))
  add('an updater key that is not a key', doc(1, [1], [], { updater_keys: ['%%%'] }))
  add('an updater key both allowed and revoked', doc(1, [1], [], { updater_keys: [pair.pubkey], revoked_updater_keys: [pair.idHex] }))
  add('expired', doc(1, [1], []), rootSeed, parseUtc('2027-01-15T10:00:00Z') as number)
  add('the last second', doc(1, [1], []), rootSeed, (parseUtc('2027-01-15T10:00:00Z') as number) - 1)
  statementCases.push({ name: 'a signature that is not base64', doc: base, sig: '***', now: NOW })
  statementCases.push({ name: 'an empty signature', doc: base, sig: '', now: NOW })
  statementCases.push({ name: 'a signature of the wrong length', doc: base, sig: 'AAAA', now: NOW })
  statementCases.push({ name: 'a tampered document', doc: base.replace('2026-10-17', '2026-10-18'), sig: signText(testKey(rootSeed), base), now: NOW })
  statementCases.push({ name: 'a signature with whitespace around it', doc: base, sig: ` ${signText(testKey(rootSeed), base)}\n`, now: NOW })

  const root = rootKey(rootPub)
  const statements = statementCases.map((c) => {
    let verify: Json
    let accept: Json | string
    try {
      const st = verifyStatement(Buffer.from(c.doc), Buffer.from(c.sig), root)
      verify = {
        ok: {
          serial: st.serial,
          bundle_keys: statementBundleKeys(st).map((k) => k.id),
          caps: Object.fromEntries([...statementCaps(st)].map(([k, v]) => [String(k), formatVersion(v)])),
          updater_keys: st.updater_keys.length,
        },
      }
    } catch (error) {
      verify = { error: (error as Error).message }
    }
    try {
      accept = TrustState.load(null, root).accept(Buffer.from(c.doc), Buffer.from(c.sig), c.now)
    } catch (error) {
      accept = { error: (error as Error).message }
    }
    return { ...c, verify, accept }
  })

  const sequences = [
    {
      name: 'an older serial is refused and an equal one changes nothing',
      steps: [signedBy(rootSeed, doc(2, [2], [1])), signedBy(rootSeed, doc(1, [1], [])), signedBy(rootSeed, doc(2, [2], [1])), signedBy(rootSeed, doc(2, [3], [])), signedBy(rootSeed, doc(3, [3], [1]))],
    },
    { name: 'a statement signed by another key never takes hold', steps: [signedBy(8, doc(1, [1], [])), signedBy(rootSeed, doc(1, [1], []))] },
  ].map((seq) => {
    const trust = TrustState.load(null, root)
    return {
      name: seq.name,
      steps: seq.steps.map((s) => {
        let expect: Json | string
        try {
          expect = trust.accept(Buffer.from(s.doc), Buffer.from(s.sig), NOW)
        } catch (error) {
          expect = { error: (error as Error).message }
        }
        return { ...s, now: NOW, expect, serial: trust.statement()?.serial ?? null }
      }),
    }
  })

  const times = ['1970-01-01T00:00:00Z', '2026-11-01T00:00:00Z', '2024-02-29T12:30:45Z', '2000-01-01T00:00:00Z', '2026-11-01 00:00:00Z', '2026-11-01T00:00:00+00:00', '2026-13-01T00:00:00Z', '2026-11-32T00:00:00Z',
    '2026-11-01T24:00:00Z', '2026-11-01T00:60:00Z', '2026-11-01T00:00:60Z', '2026-1x-01T00:00:00Z', '1969-12-31T23:59:59Z', '2026-00-10T00:00:00Z', '2026-01-00T00:00:00Z', '']
    .map((input) => ({ input, secs: parseUtc(input) }))

  const sigOf = (m: ReturnType<typeof minisignKey>) => minisignSignature(m, Buffer.from('installer'), '1.0.0')
  const ids = [
    { kind: 'key', input: pair.pubkey }, { kind: 'key', input: pair2.pubkey }, { kind: 'key', input: '%%%' }, { kind: 'key', input: '' },
    { kind: 'sig', input: sigOf(pair) }, { kind: 'sig', input: sigOf(pair2) }, { kind: 'sig', input: 'garbage' }, { kind: 'sig', input: '' },
    { kind: 'sig', input: minisignSignature(pair, Buffer.from('x'), '1.0.0', { prehashed: false }) },
  ].map((c) => ({ ...c, id: c.kind === 'key' ? minisignKeyId(c.input) : signatureKeyId(c.input) }))

  const st = (allowed: string[], revoked: string[]) => ({ updater_keys: allowed, revoked_updater_keys: revoked })
  const choices: { name: string, pinned: string, statement: ReturnType<typeof st> | null, signature: string }[] = [
    { name: 'no statement: the pinned key decides', pinned: pair.pubkey, statement: null, signature: sigOf(pair2) },
    { name: 'listed and not revoked', pinned: pair.pubkey, statement: st([pair.pubkey, pair2.pubkey], []), signature: sigOf(pair) },
    { name: 'a statement introduces a key', pinned: pair.pubkey, statement: st([pair.pubkey, pair2.pubkey], []), signature: sigOf(pair2) },
    { name: 'revoked, even the pinned one', pinned: pair.pubkey, statement: st([pair2.pubkey], [pair.idHex]), signature: sigOf(pair) },
    { name: 'a replacement of a revoked key', pinned: pair.pubkey, statement: st([pair2.pubkey], [pair.idHex]), signature: sigOf(pair2) },
    { name: 'not listed', pinned: pair.pubkey, statement: st([pair2.pubkey], []), signature: sigOf(pair) },
    { name: 'a malformed signature', pinned: pair.pubkey, statement: st([pair2.pubkey], []), signature: 'garbage' },
    { name: 'no updater keys listed', pinned: pair.pubkey, statement: st([], []), signature: sigOf(pair) },
    { name: 'no updater keys listed, one revoked', pinned: pair.pubkey, statement: st([], [pair.idHex]), signature: sigOf(pair) },
  ]
  const updaterChoices = choices.map((c) => {
    const full = c.statement && { schema: 1, serial: 2, issued_at: 'x', expires_at: 'y', bundle_keys: [], revoked_bundle_keys: [], max_version: {}, ...c.statement }
    const r = chooseUpdaterKey(c.pinned, full, c.signature)
    return { ...c, expect: r.ok ? { key: r.key } : { refusal: r.refusal } }
  })

  return { root: rootPub, now: NOW, times, statements, sequences, minisign_ids: ids, updater_choices: updaterChoices }
}

// ------------------------------------------------------------------- proxy

function proxyVectors(): Json {
  const types = ['application/json', 'text/plain; charset=utf-8', 'image/png', 'IMAGE/JPEG', 'image/webp', 'image/avif', 'image/x-icon', 'audio/ogg', 'video/mp4', 'application/vnd.api+json', 'application/octet-stream',
    'text/html', 'image/svg+xml', 'application/xml', 'text/javascript', 'application/javascript', 'application/pdf', 'text/xml', 'application/xhtml+xml', 'text/css', 'application/json+x', '', 'multipart/x-mixed-replace']
    .map((input) => ({ input, safe: inlineSafe(input) }))
  const origin = 'https://community.example'
  const targets = ['/api/channels?x=1', '/api/', '/index.html', '/api', '/api/../x', '/api/%2e%2e/x', '/api/a\\b', '/api/a\nb', '//evil.example/api/x', '/apix', 'https://evil.example/api/x', '/api/\u0085', '/api/a b', '/api/%2Fx', '/api/./x']
    .map((input) => ({ origin, input, href: target(new URL(origin), input)?.href ?? null }))
  const upload = '/api/channels/00000000-0000-4000-8000-000000000000/upload'
  const capSteps = [
    { announce: [], path: upload }, { announce: [10], path: upload }, { announce: [10], path: '/api/channels/x/messages' }, { announce: [10], path: '/api/users/me/avatar' },
    { announce: [10], path: '/api/channels//upload' }, { announce: [10], path: '/api/channels/a/b/upload' }, { announce: [512], path: upload }, { announce: [512, 0, 999_999], path: upload }, { announce: [2048], path: upload },
    { announce: [1], path: upload }, { announce: [2049], path: upload },
  ].map((c) => {
    const limits = new Limits()
    for (const mb of c.announce) limits.announceUploadMb(mb)
    return { ...c, request_cap: limits.requestCap(c.path), response_cap: limits.responseCap() }
  })
  const mimes = ['index.html', 'a.JS', 'a.mjs', 'a.css', 'a.json', 'a.map', 'a.wasm', 'a.svg', 'a.png', 'a.jpg', 'a.jpeg', 'a.webp', 'a.gif', 'a.ico', 'a.woff2', 'a.woff', 'a.txt', 'a.webmanifest', 'a.mp3', 'a.ogg', 'a.oga', 'a.wav', 'a.bin', 'noext', 'a.tar.gz']
    .map((name) => ({ name, type: mimeFor(name) }))

  const sb = spec('0.8.0', { files: [['index.html', '<html>app</html>'], ['assets/app-1.js', 'console.log(1)'], ['logo.svg', '<svg/>'], ['sw.js', 'x']] })
  const zip = bundle(sb)
  const served = verifyBundle(zip, trusted(7))
  const serveCases = [['GET', '/'], ['GET', '/index.html'], ['GET', '/assets/app-1.js'], ['GET', '/assets/missing.js'], ['GET', '/c/11111111-1111-1111-1111-111111111111'], ['GET', '//index.html'], ['GET', '/logo.svg'],
    ['GET', '/sw.js'], ['HEAD', '/'], ['HEAD', '/assets/app-1.js'], ['POST', '/'], ['DELETE', '/assets/app-1.js'], ['GET', '/nothing/here']]
    .map(([method, path]) => {
      const r = serveBundle(served, method as string, path as string)
      const h = (n: string) => r.headers.find(([k]) => k === n)?.[1] ?? null
      return { method, path, status: r.status, content_type: h('content-type'), cache_control: h('cache-control'), allow: h('allow'), body: Buffer.from(r.body).toString() }
    })

  const jarSteps: { name: string, steps: Json[] }[] = [
    {
      name: 'prefixes, the secure flag, sizes and names',
      steps: [
        { op: 'store', https: false, now: 0, set_cookie: ['__Host-a=1; Max-Age=60', '__Host-b=1; Secure; Path=/x; Max-Age=60', '__Host-c=1; Secure; Path=/; Domain=example.org; Max-Age=60', '__Secure-d=1; Max-Age=60', 'e=1; Secure; Max-Age=60', '=nameless; Max-Age=60', 'not a cookie'] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: ['__Host-ok=1; Secure; Path=/; Max-Age=60'] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: [`big=${'x'.repeat(5000)}; Max-Age=60`] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: ['__Secure-s=2; Secure; Max-Age=60', 'plain=3; Max-Age=60; HttpOnly; SameSite=Lax'] },
        { op: 'header', now: 1 },
      ],
    },
    {
      name: 'expiry, replacement and logout',
      steps: [
        { op: 'store', https: true, now: 100, set_cookie: ['keep=1; Max-Age=1000', 'gone=1; Max-Age=10', 'session=1', 'late=1; Max-Age=500'] },
        { op: 'header', now: 101 },
        { op: 'header', now: 111 },
        { op: 'header', now: 1101 },
        { op: 'store', https: true, now: 200, set_cookie: ['keep=2; Max-Age=1000'] },
        { op: 'header', now: 201 },
        { op: 'store', https: true, now: 300, set_cookie: ['keep=; Max-Age=0', 'session=; Max-Age=0'] },
        { op: 'header', now: 301 },
      ],
    },
  ]
  const jars = jarSteps.map((j) => {
    const jar = new Jar(null, null, 0)
    return {
      name: j.name,
      steps: j.steps.map((s) => {
        if (s.op === 'store') { jar.store(s.set_cookie as string[], s.https as boolean, s.now as number); return s }
        return { ...s, expect: jar.header(s.now as number) }
      }),
    }
  })

  return { inline_safe: types, targets, caps: capSteps, mimes, serve: { trusted: keysOf(7), zip: b64(zip), cases: serveCases }, jars }
}

// ------------------------------------------------------------------ client

function clientVectors(): Json {
  const addresses = ['community.example', 'https://community.example/', 'https://community.example:8443', ' community.example ', 'COMMUNITY.example', 'http://localhost:8080', 'http://127.0.0.1', 'http://127.1', 'http://[::1]:3000',
    'http://LOCALHOST', '', 'http://community.example', 'http://127.0.0.1.example', 'http://192.168.1.10:8080', 'https://user@community.example', 'https://user:pw@community.example', 'https://community.example/path',
    'https://community.example:8443/?invite=code', 'http://community.example:8443/path?invite=code', 'http://localhost:8080/path?x=1',
    'https://community.example?token=x', 'https://community.example/#x', 'https://community.example\\other', 'https://community.example\n', 'https://community.example/?', 'https://a@b.example', 'ftp://community.example',
    'https://', 'https://exa mple.com', 'http://0.0.0.0', 'http://[::]', 'https://community.example:99999']
    .map((input) => {
      try { return { input, href: parseAddress(input).href } } catch (error) { return { input, error: (error as Error).message } }
    })
  const healths = ['{"status":"ok","version":"0.7.17","web_client_api":1}', '{"status":"ok","version":"dev","web_client_api":3}', '{"status":"ok","version":"0.7.0"}', '{"status":"ok","version":"0.7.0","web_client_api":0}',
    'not JSON', '[]', '{"status":"ok","version":null,"web_client_api":1}', '{"status":"unavailable","web_client_api":1}', '{"status":"ok","web_client_api":2}', '{"status":"ok","version":"0.7.0","web_client_api":"1"}',
    '{"status":"ok","version":"0.7.0","web_client_api":null}', '{"status":"ok","version":"0.7.0","web_client_api":-1}', '{"status":"down"}', '{}']
    .map((input) => {
      try {
        const info = parseHealth(Buffer.from(input))
        return { input, ok: { version: info.version === null ? null : formatVersion(info.version), api: info.webClientApi } }
      } catch (error) { return { input, error: (error as Error).message } }
    })
  const externals = ['https://example.invalid/page?q=value', 'file:///tmp/example', 'javascript:alert(1)', 'mailto:user@example.invalid', 'https://user:password@example.invalid/', 'http://example.invalid/', 'https://example.invalid/' + 'a'.repeat(5000)]
    .map((url) => ({ url, allowed: externalAllowed(new URL(url)) }))
  const ID = '11111111-1111-1111-1111-111111111111'
  const paths = ['', '/', '/index.html', '/login', '/admin', '/admin/users', '/admin/channels', '/admin/invites', '/admin/media', '/admin/system', `/c/${ID}`, `/c/${ID}/m/${ID}`, `/c/${ID}/t/${ID}`, `/v/${ID}`, `/v/${ID}/chat`,
    `/v/${ID}/chat/m/${ID}`, '/other.html', '/native-media.html', '/admin/unknown', '/admin/users/extra', '/c/not-an-id', '/c/00000000-0000-0000-0000-000000000000', `/c/${ID}/`, `/c/${ID}/m`, `/v/${ID}/chat/t/${ID}`,
    `/c/${ID}%2fm%2f${ID}`, `//c/${ID}`, '/index.html/extra', '/INDEX.HTML', '/c/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA', '/c/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', `/${'a'.repeat(201)}`, '/admin/', '/login/']
    .map((path) => ({ path, ok: appPath(path) }))
  return { addresses, healths, externals, app_paths: paths }
}

// ---------------------------------------------------------------- minisign

function minisignVectors(): Json {
  const key = minisignKey(3)
  const other = minisignKey(4)
  const pkg = 'package bytes'
  const content = Buffer.from(pkg)
  const sign = (m = key, bytes: Uint8Array = content, version = '1.0.0', options: Parameters<typeof minisignSignature>[3] = {}) => minisignSignature(m, bytes, version, options)
  const good = sign()
  const tamperedComment = Buffer.from(Buffer.from(good, 'base64').toString('utf8').replace('version:1.0.0', 'version:9.0.0')).toString('base64')
  const cases: { name: string, package: string, signature: string, pubkey: string, version: string }[] = [
    { name: 'a prehashed signature', package: pkg, signature: good, pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a legacy signature', package: pkg, signature: sign(key, content, '1.0.0', { prehashed: false }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a version with a v in the signed comment', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1\tversion:v1.0.0' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a version among other fields', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1\tfile:x\tversion:1.0.0\textra:y' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'another package', package: 'package bytez', signature: good, pubkey: key.pubkey, version: '1.0.0' },
    { name: 'another key', package: pkg, signature: good, pubkey: other.pubkey, version: '1.0.0' },
    { name: 'another announced version', package: pkg, signature: good, pubkey: key.pubkey, version: '1.0.1' },
    { name: 'an announced version that is not a version', package: pkg, signature: good, pubkey: key.pubkey, version: 'x' },
    { name: 'a key that is not a key', package: pkg, signature: good, pubkey: '%%%', version: '1.0.0' },
    { name: 'a signature that is not a signature', package: pkg, signature: 'garbage', pubkey: key.pubkey, version: '1.0.0' },
    { name: 'no version in the signed comment', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a trusted comment edited after signing', package: pkg, signature: tamperedComment, pubkey: key.pubkey, version: '9.0.0' },
    { name: 'a signature naming another key id', package: pkg, signature: sign(key, content, '1.0.0', { idBytes: Buffer.alloc(8, 9) }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a signature of another key under the right id', package: pkg, signature: sign(other, content, '1.0.0', { idBytes: key.idBytes }), pubkey: key.pubkey, version: '1.0.0' },
  ]
  const real = {
    name: 'a signature written by the Tauri CLI',
    package: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.bin'), 'utf8'),
    signature: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.sig'), 'utf8').trim(),
    pubkey: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.pub'), 'utf8').trim(),
    version: '1.2.3',
  }
  const all = [...cases, real, { ...real, name: 'a signature written by the Tauri CLI, another version', version: '1.2.4' }]
  return {
    cases: all.map((c) => ({ ...c, expect: verifyPackage(Buffer.from(c.package), c.signature, c.pubkey, c.version) })),
  }
}

// ------------------------------------------------------------------- main

const files: Record<string, Json> = {
  'bundle.json': bundleVectors(),
  'store.json': storeVectors(),
  'trust.json': trustVectors(),
  'proxy.json': proxyVectors(),
  'client.json': clientVectors(),
  'minisign.json': minisignVectors(),
}
const check = process.argv.includes('--check')
let drift = false
for (const [name, content] of Object.entries(files)) {
  const text = `${JSON.stringify({ format: 1, note: 'Generated by linux/scripts/gen-vectors.ts; read by the Rust host and the TypeScript client.', ...content }, null, 1)}\n`
  const path = join(root, name)
  if (check) {
    let current = ''
    try { current = readFileSync(path, 'utf8') } catch { /* missing */ }
    if (current !== text) { console.error(`test-vectors/${name} is out of date: run npm run vectors in linux/`); drift = true }
  } else {
    mkdirSync(root, { recursive: true })
    writeFileSync(path, text)
    console.log(`wrote test-vectors/${name} (${text.length} bytes)`)
  }
}
if (drift) process.exit(1)
