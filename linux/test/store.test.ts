import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, unlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { CHECK_INTERVAL_SECS, Store, installCurrent, refusalText, type ServerInfo } from '../src/core/store.ts'
import { bundle, spec, trusted, type Spec } from './helpers/fixtures.ts'

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
function dir(): string {
  const d = mkdtempSync(join(tmpdir(), 'mnema-store-'))
  dirs.push(d)
  return d
}

const server = (version: string | null, api = 1): ServerInfo => ({ version: version === null ? null : (version.split('.').map(Number) as [number, number, number]), webClientApi: api })
const dev: ServerInfo = { version: null, webClientApi: 1 }

function selected(store: Store, info: ServerInfo): string {
  const result = store.select(info)
  assert.ok(result.ok, JSON.stringify(result))
  return result.held.bundle.version.join('.')
}

test('the server is matched by contract and release', () => {
  const next = spec('0.9.0', { minServer: '0.8.9' })
  const old = spec('0.8.0', { minServer: '0.7.0' })
  const store = Store.load(null, bundle(old), trusted(7))
  store.install(bundle(next), trusted(7))
  assert.equal(selected(store, server('0.9.0')), '0.9.0')
  // A server that claims an older release does not get the older bundle.
  assert.deepEqual(store.select(server('0.8.5')), { ok: false, refusal: { kind: 'server_too_old', needs: [0, 8, 9] } })
  assert.equal(selected(store, dev), '0.9.0')
  assert.deepEqual(store.select(server('0.6.0')), { ok: false, refusal: { kind: 'server_too_old', needs: [0, 8, 9] } })
  assert.deepEqual(store.select(server('0.9.0', 2)), { ok: false, refusal: { kind: 'server_newer' } })
  const api2 = Store.load(null, bundle(spec('1.0.0', { api: 2 })), trusted(7))
  assert.deepEqual(api2.select(dev), { ok: false, refusal: { kind: 'server_contract_retired' } })
  assert.deepEqual(Store.load(null, null, trusted(7)).select(dev), { ok: false, refusal: { kind: 'no_bundle' } })
  for (const kind of ['no_bundle', 'server_newer', 'server_contract_retired', 'rolled_back', 'signer_withdrawn'] as const) {
    assert.ok(refusalText({ kind }).length > 0)
  }
  assert.match(refusalText({ kind: 'server_too_old', needs: [0, 7, 0] }), /needs 0\.7\.0 or newer/)
})

test('the high-water mark blocks older bundles even without a floor', () => {
  const d = dir()
  const old = spec('0.8.0')
  const store = Store.load(d, bundle(old), trusted(7))
  store.install(bundle(spec('0.8.4')), trusted(7))
  assert.deepEqual(store.floor(), [0, 0, 0])
  assert.deepEqual(store.highWater(1), [0, 8, 4])
  const lie = server('0.1.0')
  // Whatever the server claims, the built-in 0.8.0 is not offered.
  assert.equal(selected(store, lie), '0.8.4')
  // The mark survives a restart and the loss of the newer bundle: the client refuses instead of running the older one.
  unlinkSync(join(d, 'mnema-web-0.8.4.zip'))
  const again = Store.load(d, bundle(old), trusted(7))
  assert.deepEqual(again.highWater(1), [0, 8, 4])
  assert.deepEqual(again.select(lie), { ok: false, refusal: { kind: 'rolled_back' } })
  // A lost state file is rebuilt from what verifies, never lowered.
  unlinkSync(join(d, 'state.json'))
  assert.deepEqual(Store.load(d, bundle(old), trusted(7)).highWater(1), [0, 8, 0])
  // Another contract is tracked on its own.
  const other = Store.load(d, bundle(old), trusted(7))
  other.install(bundle(spec('0.9.0', { api: 2 })), trusted(7))
  assert.deepEqual(other.highWater(1), [0, 8, 0])
  assert.deepEqual(other.highWater(2), [0, 9, 0])
  assert.equal(selected(other, dev), '0.8.0')
})

function both(): ReturnType<typeof trusted> { return [...trusted(7), ...trusted(5)] }

function forgedAndGood(d: string): Store {
  const forged = spec('0.99.0', { seed: 5, floor: '0.99.0' })
  const store = Store.load(d, bundle(spec('0.8.0')), both())
  store.install(bundle(forged), both())
  return store
}

test('a forged mark of a revoked key is cleared', () => {
  const d = dir()
  const store = forgedAndGood(d)
  assert.deepEqual(store.floor(), [0, 99, 0])
  assert.equal(store.reconcile(trusted(7)), true)
  assert.equal(existsSync(join(d, 'mnema-web-0.99.0.zip')), false)
  assert.deepEqual(store.floor(), [0, 0, 0])
  assert.deepEqual(store.highWater(1), [0, 8, 0])
  // The good built-in bundle, held back by the forged floor, runs again.
  assert.equal(selected(store, dev), '0.8.0')
  assert.equal(store.reconcile(trusted(7)), false)
})

test('the cleared marks stay cleared after a restart', () => {
  const d = dir()
  forgedAndGood(d)
  const again = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  assert.equal(again.rejected.length, 1)
  assert.deepEqual(again.floor(), [0, 0, 0])
  assert.deepEqual(again.highWater(1), [0, 8, 0])
  assert.equal(selected(again, dev), '0.8.0')
  // With both keys trusted the forged marks are in force again.
  assert.deepEqual(Store.load(d, bundle(spec('0.8.0')), both()).floor(), [0, 99, 0])
})

test('marks and floors of still trusted keys are kept', () => {
  const store = Store.load(dir(), bundle(spec('0.8.0')), both())
  store.install(bundle(spec('0.9.0', { floor: '0.8.5' })), both())
  store.install(bundle(spec('0.99.0', { seed: 5, floor: '0.99.0' })), both())
  assert.equal(store.reconcile(trusted(7)), true)
  assert.deepEqual(store.floor(), [0, 8, 5])
  assert.deepEqual(store.highWater(1), [0, 9, 0])
  assert.equal(selected(store, dev), '0.9.0')
  // The built-in 0.8.0 stays below the trusted floor.
  assert.equal(store.held().length, 1)
})

test('when only revoked bundles are left the client says so until a new one arrives', () => {
  const forged = spec('0.8.0', { seed: 5 })
  const store = Store.load(null, bundle(forged), trusted(5))
  assert.ok(store.select(dev).ok)
  assert.equal(store.reconcile(trusted(7)), true)
  assert.deepEqual(store.select(dev), { ok: false, refusal: { kind: 'signer_withdrawn' } })
  assert.match(refusalText({ kind: 'signer_withdrawn' }), /no longer trusted/)
  store.install(bundle(spec('0.8.1')), trusted(7))
  assert.equal(selected(store, dev), '0.8.1')
  // An offline start with a built-in bundle nobody trusts any more.
  assert.deepEqual(Store.load(null, bundle(forged), trusted(7)).select(dev), { ok: false, refusal: { kind: 'signer_withdrawn' } })
})

test('a cap from the statement lowers marks of trusted signers and drops bundles above it', () => {
  // Marks recorded by a state file of the new format: a forged 0.99.0 mark of a key that is still trusted.
  const d = dir()
  const store = Store.load(d, bundle(spec('0.8.0')), both())
  store.install(bundle(spec('0.99.0', { seed: 5, floor: '0.99.0' })), both())
  assert.deepEqual(store.floor(), [0, 99, 0])
  assert.equal(store.select(dev).ok, true)
  store.applyCaps(new Map([[1, [0, 8, 0]]]))
  assert.deepEqual(store.floor(), [0, 8, 0])
  assert.deepEqual(store.highWater(1), [0, 8, 0])
  assert.equal(selected(store, dev), '0.8.0')
  // The cap survives a restart in the recorded marks (the dropped bundle's file is gone).
  unlinkSync(join(d, 'mnema-web-0.99.0.zip'))
  assert.deepEqual(Store.load(d, bundle(spec('0.8.0')), both()).floor(), [0, 8, 0])
  // Other contracts keep their own marks; no caps change nothing.
  const high = spec('0.99.0', { floor: '0.5.0' })
  const api2 = spec('1.0.0', { api: 2 })
  const other = Store.load(null, bundle(spec('0.8.0')), trusted(7))
  other.install(bundle(high), trusted(7))
  other.install(bundle(api2), trusted(7))
  other.applyCaps(new Map())
  assert.deepEqual(other.highWater(1), [0, 99, 0])
  other.applyCaps(new Map([[1, [0, 9, 0]]]))
  assert.deepEqual(other.highWater(1), [0, 9, 0])
  assert.deepEqual(other.highWater(2), [1, 0, 0])
  assert.ok(other.held().filter((h) => h.bundle.webClientApi === 1).every((h) => h.bundle.version[1] <= 9 && h.bundle.version[0] === 0))
})

test('installs use the keys in force when they run', () => {
  const store = Store.load(null, null, both())
  let keys = both()
  const forged = spec('0.8.0', { seed: 5 })
  // A statement withdrawing key 5 lands after the keys were first read.
  keys = trusted(7)
  assert.throws(() => installCurrent(store, () => keys, bundle(forged)))
  assert.equal(store.held().length, 0)
  assert.doesNotThrow(() => installCurrent(store, () => keys, bundle(spec('0.8.0'))))
})

test('the security floor only moves up and withdraws older bundles', () => {
  const d = dir()
  const old = spec('0.8.0')
  const fixed = spec('0.8.2', { floor: '0.8.1' })
  const store = Store.load(d, bundle(old), trusted(7))
  assert.deepEqual(store.floor(), [0, 0, 0])
  store.install(bundle(fixed), trusted(7))
  assert.deepEqual(store.floor(), [0, 8, 1])
  // The built-in 0.8.0 is below the floor now and cannot be selected.
  assert.equal(store.held().length, 1)
  assert.deepEqual(store.newest()?.bundle.version, [0, 8, 2])
  // The floor survives a restart, even if the bundle that raised it is gone.
  unlinkSync(join(d, 'mnema-web-0.8.2.zip'))
  const again = Store.load(d, bundle(old), trusted(7))
  assert.deepEqual(again.floor(), [0, 8, 1])
  assert.equal(again.newest(), undefined)
  // Nor can an old bundle come back through an update.
  assert.throws(() => Store.load(d, null, trusted(7)).install(bundle(old), trusted(7)), /below the security floor/)
})

test('updates install only newer verified bundles and survive a restart', () => {
  const d = dir()
  const store = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  assert.throws(() => store.install(bundle(spec('0.8.0')), trusted(7)), /already held/)
  assert.throws(() => store.install(bundle(spec('0.7.0')), trusted(7)), /already held|floor/)
  assert.throws(() => store.install(Buffer.from('garbage'), trusted(7)))
  assert.throws(() => store.install(bundle(spec('0.9.0', { seed: 9 })), trusted(7)), /does not match a trusted key/)
  assert.deepEqual(store.install(bundle(spec('0.8.1')), trusted(7)), [0, 8, 1])
  assert.equal(store.newest()?.source, 'cache')
  const reloaded = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  assert.equal(reloaded.held().length, 2)
  assert.deepEqual(reloaded.newest()?.bundle.version, [0, 8, 1])
})

test('a modified cached bundle is refused and the next verified one is used', () => {
  const d = dir()
  const store = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  store.install(bundle(spec('0.8.1')), trusted(7))
  const path = join(d, 'mnema-web-0.8.1.zip')
  const bytes = readFileSync(path)
  const at = bytes.indexOf('console.log(1)')
  assert.ok(at > 0)
  bytes[at] ^= 1
  writeFileSync(path, bytes)
  const reloaded = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  assert.deepEqual(reloaded.rejected, ['mnema-web-0.8.1.zip'])
  assert.equal(reloaded.held().length, 1)
  assert.equal(reloaded.newest()?.source, 'builtin')
  assert.deepEqual(reloaded.newest()?.bundle.version, [0, 8, 0])
  // And the damaged file does not stop the update from replacing it.
  assert.deepEqual(reloaded.install(bundle(spec('0.8.1')), trusted(7)), [0, 8, 1])
})

test('a broken built-in bundle is reported and unusable', () => {
  const store = Store.load(null, Buffer.from('not a bundle'), trusted(7))
  assert.deepEqual(store.rejected, ['built-in'])
  assert.equal(store.newest(), undefined)
})

test('the cache keeps only the newest few versions and ignores strangers', () => {
  const d = dir()
  writeFileSync(join(d, 'notes.txt'), 'x')
  writeFileSync(join(d, 'mnema-web-9.9.9.zip'), Buffer.alloc(10))
  const store = Store.load(d, bundle(spec('0.8.0')), trusted(7))
  for (const v of ['0.8.1', '0.8.2', '0.8.3', '0.8.4']) store.install(bundle(spec(v)), trusted(7))
  assert.deepEqual(store.held().map((h) => h.bundle.version.join('.')), ['0.8.4', '0.8.3', '0.8.2', '0.8.0'])
  assert.equal(existsSync(join(d, 'mnema-web-0.8.1.zip')), false)
  assert.ok(readdirSync(d).includes('notes.txt'))
})

test('update checks are rate limited and remembered', () => {
  const d = dir()
  const store = Store.load(d, null, trusted(7))
  assert.equal(store.checkDue(1_000_000), true)
  store.markChecked(1_000_000)
  assert.equal(store.checkDue(1_000_000 + CHECK_INTERVAL_SECS - 1), false)
  assert.equal(store.checkDue(1_000_000 + CHECK_INTERVAL_SECS), true)
  assert.equal(Store.load(d, null, trusted(7)).checkDue(1_000_001), false)
})

test('a spec can be built for any version', () => {
  const s: Spec = spec('1.2.3')
  assert.equal(s.version, '1.2.3')
})
