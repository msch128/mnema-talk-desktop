import { existsSync, mkdtempSync, rmSync, unlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { parseKey } from '../../src/core/bundle.ts'
import { Store } from '../../src/core/store.ts'
import { formatVersion, parseVersion } from '../../src/core/version.ts'
import { bundle, spec, type Spec } from '../../test/helpers/fixtures.ts'
import { b64, hex, type Json } from './common.ts'

type Step = Json

const srv = (version: string | null, api = 1) => ({ version, api })
const install = (name: string, trustedKeys: string[]): Step => ({ op: 'install', bundle: name, trusted: trustedKeys })
const select = (server: Json): Step => ({ op: 'select', server })
const state: Step = { op: 'state' }

/** The bundles the scenarios install, by short name. */
function namedSpecs(): Record<string, Spec> {
  return {
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
}

type Scenario = { name: string, persist: boolean, builtin: string | null, trusted: string[], steps: Step[] }

/** The scenarios, without the answers the TypeScript store gives. */
function scenarioList(): Scenario[] {
  return [
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
}

export function storeVectors(): Json {
  const named = namedSpecs()
  const bundles = Object.fromEntries(Object.entries(named).map(([k, v]) => [k, b64(bundle(v))]))
  const keys = { a: hex(7), b: hex(5), c: hex(9) }
  // Replay the scenarios on the TypeScript store to write what it answers. The Rust
  // host replays the same file; a difference is a bug on one side.
  const out = scenarioList().map((s) => ({ ...s, steps: replay(s, named, keys) }))
  return { keys, bundles, scenarios: out }
}

function trustedOf(names: string[], keys: Record<string, string>) {
  return names.map((n) => parseKey(keys[n] as string) as NonNullable<ReturnType<typeof parseKey>>)
}

/** What one scenario run holds while its steps are replayed. */
type Run = { store: Store, dir: string | null, builtin: Buffer | null, named: Record<string, Spec>, keys: Record<string, string> }

const handlers: Record<string, (run: Run, step: Step) => Step> = {
  install(run, step) {
    let expect: Json
    try {
      expect = { ok: formatVersion(run.store.install(bundle(run.named[step.bundle as string] as Spec), trustedOf(step.trusted as string[], run.keys))) }
    } catch (error) {
      expect = { error: (error as Error).message }
    }
    return { ...step, expect }
  },
  select(run, step) {
    const server = step.server as { version: string | null, api: number }
    const result = run.store.select({ version: server.version === null ? null : parseVersion(server.version), webClientApi: server.api })
    return {
      ...step,
      expect: result.ok ? { ok: formatVersion(result.held.bundle.version) }
        : { refusal: result.refusal.kind, ...(result.refusal.kind === 'server_too_old' ? { needs: formatVersion(result.refusal.needs) } : {}) },
    }
  },
  reconcile: (run, step) => ({ ...step, expect: run.store.reconcile(trustedOf(step.trusted as string[], run.keys)) }),
  caps(run, step) {
    const caps = new Map(Object.entries(step.caps as Record<string, string>).map(([k, v]) => [Number(k), parseVersion(v) as NonNullable<ReturnType<typeof parseVersion>>]))
    run.store.applyCaps(caps)
    return step
  },
  state({ store }, step) {
    const highWater: Record<string, string> = {}
    for (const api of [1, 2, 3]) {
      const v = store.highWater(api)
      if (v) highWater[String(api)] = formatVersion(v)
    }
    return { ...step, expect: { floor: formatVersion(store.floor()), high_water: highWater, held: store.held().map((h) => `${formatVersion(h.bundle.version)}:${h.source}`), rejected: store.rejected.length } }
  },
  restart(run, step) {
    run.store = Store.load(run.dir, run.builtin, trustedOf(step.trusted as string[], run.keys))
    return step
  },
  remove_cached(run, step) {
    unlinkSync(join(run.dir as string, `mnema-web-${step.version as string}.zip`))
    return step
  },
  forget_state(run, step) {
    if (existsSync(join(run.dir as string, 'state.json'))) unlinkSync(join(run.dir as string, 'state.json'))
    return step
  },
}

/** Runs the steps on the TypeScript store and adds the `expect` of each. */
function replay(s: Scenario, named: Record<string, Spec>, keys: Record<string, string>): Step[] {
  const dir = s.persist ? mkdtempSync(join(tmpdir(), 'mnema-vectors-')) : null
  const builtin = s.builtin ? bundle(named[s.builtin] as Spec) : null
  const run: Run = { store: Store.load(dir, builtin, trustedOf(s.trusted, keys)), dir, builtin, named, keys }
  try {
    return s.steps.map((step) => {
      const handler = handlers[step.op as string]
      if (!handler) throw new Error(`unknown step ${String(step.op)}`)
      return handler(run, step)
    })
  } finally {
    if (dir) rmSync(dir, { recursive: true, force: true })
  }
}
