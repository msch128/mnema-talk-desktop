import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { PINNED_BUNDLE_KEYS, ROOT_PUB, UPDATER_PUBKEY } from '../src/core/pinned.ts'
import { parseKey } from '../src/core/bundle.ts'
import { type Floor, type Statement, TrustState, chooseUpdaterKey, minisignKeyId, parseUtc, rootKey, signatureKeyId, statementBundleKeys, statementCaps, statementExpired, verifyStatement } from '../src/core/trust.ts'
import { minisignKey, minisignSignature, signText, statementText, testKey, trusted, type StatementSpec } from './helpers/fixtures.ts'

const NOW = 1_793_491_200 // 2026-11-01T00:00:00Z

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
function dir(): string {
  const d = mkdtempSync(join(tmpdir(), 'mnema-trust-'))
  dirs.push(d)
  return d
}

const root = (seed: number) => rootKey(testKey(seed).hex)

function doc(serial: number, allowed: number[], revoked: number[]): string {
  return statementText({
    serial,
    issued_at: '2026-10-17T10:00:00Z',
    expires_at: '2027-01-15T10:00:00Z',
    bundle_keys: allowed.map((s) => testKey(s).hex),
    revoked_bundle_keys: revoked.map((s) => testKey(s).id),
  })
}

function signed(seed: number, text: string): [Buffer, Buffer] {
  return [Buffer.from(text), Buffer.from(signText(testKey(seed), text))]
}

const state = (seed: number, d: string | null = null) => TrustState.load(d, root(seed))

test('the pinned root key is well formed', () => {
  assert.doesNotThrow(() => rootKey(ROOT_PUB))
})

test('the statement from the signing tool verifies with the pinned root key', () => {
  const docBytes = readFileSync(new URL('../../experimental/app/fixtures/trust.json', import.meta.url))
  const sig = readFileSync(new URL('../../experimental/app/fixtures/trust.json.sig', import.meta.url))
  const statement = verifyStatement(docBytes, sig, rootKey(ROOT_PUB))
  assert.equal(statement.serial, 3)
  const keys = statementBundleKeys(statement)
  const pinned = PINNED_BUNDLE_KEYS.map((k) => parseKey(k)?.id)
  assert.ok(pinned.every((id) => keys.some((k) => k.id === id)), 'every compiled-in key is allowed by the first statement')
  const pinnedUpdater = minisignKeyId(UPDATER_PUBKEY)
  assert.equal(pinnedUpdater, '2f6bbfae9c291e87')
  assert.ok(statement.updater_keys.some((k) => minisignKeyId(k) === pinnedUpdater))
})

test('a statement signed by the root key is accepted', () => {
  const [d, s] = signed(9, doc(1, [1], []))
  const trust = state(9)
  assert.equal(trust.accept(d, s, NOW), 'updated')
  assert.equal(trust.statement()?.serial, 1)
})

test('a statement signed by another key is refused', () => {
  const [d, s] = signed(8, doc(1, [1], []))
  const trust = state(9)
  assert.throws(() => trust.accept(d, s, NOW), /does not match the root key/)
  assert.equal(trust.statement(), null)
})

test('a tampered statement or signature is refused', () => {
  const [d, s] = signed(9, doc(1, [1], []))
  const trust = state(9)
  const bad = Buffer.from(d)
  bad[10] ^= 1
  assert.throws(() => trust.accept(bad, s, NOW))
  assert.throws(() => trust.accept(d, Buffer.from('AAAA'), NOW), /malformed trust signature/)
  assert.throws(() => trust.accept(d, Buffer.alloc(0), NOW), /malformed trust signature/)
  const badSig = Buffer.from(s)
  badSig[0] = badSig[0] === 0x41 ? 0x42 : 0x41
  assert.throws(() => trust.accept(d, badSig, NOW))
  assert.equal(trust.statement(), null)
})

test('a correctly signed statement must still be well formed', () => {
  const trust = state(9)
  for (const text of [
    doc(0, [1], []),
    doc(1, [], []),
    doc(1, [1], [1]),
    '{}',
    doc(1, [1], []).replace('"schema":1', '"schema":2'),
    doc(1, [1], []).replace('"serial"', '"extra":1,"serial"'),
    doc(1, [1], []).replace('2027-01-15', '2026-10-17'),
    doc(1, [1], []).replace('2027-01-15', '2029-01-15'),
    doc(1, [1], []).replace('2027-01-15T10:00:00Z', 'soon'),
  ]) {
    const [d, s] = signed(9, text)
    assert.throws(() => trust.accept(d, s, NOW), Error, text)
  }
})

test('an older serial is refused and an equal one changes nothing', () => {
  const trust = state(9)
  const [d2, s2] = signed(9, doc(2, [2], [1]))
  const [d1, s1] = signed(9, doc(1, [1], []))
  assert.equal(trust.accept(d2, s2, NOW), 'updated')
  assert.throws(() => trust.accept(d1, s1, NOW), /older/)
  assert.equal(trust.accept(d2, s2, NOW), 'unchanged')
  const [dx, sx] = signed(9, doc(2, [3], []))
  assert.throws(() => trust.accept(dx, sx, NOW), /same serial/)
  assert.equal(trust.statement()?.serial, 2)
})

test('a revoked key is dropped and a new one is allowed', () => {
  const trust = state(9)
  const pinned = trusted(1)
  assert.equal(trust.effectiveKeys(trusted(1))[0]?.id, pinned[0]?.id, 'default until a statement arrives')
  const [d, s] = signed(9, doc(2, [2], [1]))
  trust.accept(d, s, NOW)
  const keys = trust.effectiveKeys(trusted(1))
  assert.equal(keys.length, 1)
  assert.equal(keys[0]?.id, trusted(2)[0]?.id)
})

test('the last good statement survives a restart and a damaged file is ignored', () => {
  const d = dir()
  const [docBytes, sig] = signed(9, doc(3, [2], [1]))
  const trust = state(9, d)
  assert.equal(trust.statement(), null, 'offline, no statement yet')
  trust.accept(docBytes, sig, NOW)
  assert.equal(state(9, d).statement()?.serial, 3)
  // The same files under another root key are not a statement.
  assert.equal(state(8, d).statement(), null)
  const file = join(d, 'trust-state.json')
  const kept = readFileSync(file)
  const edited = Buffer.from(docBytes)
  edited[5] ^= 1
  const stored = JSON.parse(readFileSync(join(d, 'trust-state.json'), 'utf8')) as Record<string, string>
  stored.statement = edited.toString('base64')
  writeFileSync(join(d, 'trust-state.json'), JSON.stringify(stored))
  assert.equal(state(9, d).statement(), null)
  writeFileSync(file, kept.subarray(0, 10))
  assert.equal(state(9, d).statement(), null)
})

test('the statement and its signature are stored as one file', () => {
  const d = dir()
  const [docBytes, sig] = signed(9, doc(3, [2], [1]))
  state(9, d).accept(docBytes, sig, NOW)
  assert.deepEqual(readdirSync(d), ['trust-state.json'], 'no second file that could be torn from it')
})

test('the pair an older client stored is read and replaced', () => {
  const d = dir()
  const [docBytes, sig] = signed(9, doc(2, [2], [1]))
  writeFileSync(join(d, 'trust.json'), docBytes)
  writeFileSync(join(d, 'trust.json.sig'), sig)
  const trust = state(9, d)
  assert.equal(trust.statement()?.serial, 2)
  trust.accept(...signed(9, doc(3, [2], [1])), NOW)
  assert.ok(!existsSync(join(d, 'trust.json')) && !existsSync(join(d, 'trust.json.sig')))
  assert.equal(state(9, d).statement()?.serial, 3)
})

test('a verified statement applies even when it cannot be stored', () => {
  const d = join(dir(), 'trust')
  writeFileSync(d, 'in the way') // a file where the directory should be: nothing can be stored
  const logs: string[] = []
  const trust = TrustState.load(d, root(9), { log: (m) => logs.push(m) })
  const [docBytes, sig] = signed(9, doc(2, [2], [1]))
  assert.equal(trust.accept(docBytes, sig, NOW), 'updated')
  assert.equal(trust.statement()?.serial, 2)
  assert.ok(trust.effectiveKeys(trusted(1)).every((k) => k.id !== trusted(1)[0]?.id), 'the revocation is enforced in this process')
  assert.equal(logs.length, 1)
  // Once the directory works, the same fetch stores it.
  rmSync(d)
  assert.equal(trust.accept(docBytes, sig, NOW), 'unchanged')
  assert.equal(state(9, d).statement()?.serial, 2)
})

test('a later statement cannot allow a revoked key again', () => {
  const trust = state(9)
  trust.accept(...signed(9, doc(2, [2], [1])), NOW)
  // Serial 3 was signed without carrying the revocation forward.
  assert.throws(() => trust.accept(...signed(9, doc(3, [1, 2], [])), NOW), /allowed again/)
  assert.equal(trust.statement()?.serial, 2)
  // One that merely forgets to list it keeps the key revoked.
  trust.accept(...signed(9, doc(4, [2, 3], [])), NOW)
  assert.ok(trust.effectiveKeys(trusted(1)).every((k) => k.id !== testKey(1).id))
  assert.ok(trust.statement()?.revoked_bundle_keys.includes(testKey(1).id))
})

test('revocations survive a restart and apply to updater keys', () => {
  const d = dir()
  const old = minisignKey(3)
  const next = minisignKey(4)
  const withUpdaters = (serial: number, allowed: string[], revoked: string[]) => statementText({
    serial,
    issued_at: '2026-10-17T10:00:00Z',
    expires_at: '2027-01-15T10:00:00Z',
    bundle_keys: [testKey(2).hex],
    revoked_bundle_keys: [],
    updater_keys: allowed,
    revoked_updater_keys: revoked,
  })
  state(9, d).accept(...signed(9, withUpdaters(1, [next.pubkey], [old.idHex])), NOW)
  const again = state(9, d)
  assert.throws(() => again.accept(...signed(9, withUpdaters(2, [old.pubkey, next.pubkey], [])), NOW), /allowed again/)
  again.accept(...signed(9, withUpdaters(3, [next.pubkey], [])), NOW)
  assert.deepEqual(chooseUpdaterKey(old.pubkey, again.statement(), minisignSignature(old, Buffer.from('installer'), '1.0.0')), { ok: false, refusal: 'revoked' }, 'serial 3 did not list the revocation, it still holds')
})

test('the floor of a release refuses old statements and revoked keys', () => {
  const floor: Floor = { minSerial: 3, revokedBundleKeys: ['0123456789abcdef'], revokedUpdaterKeys: [] }
  const fresh = (d: string | null = null) => TrustState.load(d, root(9), { floor })
  // No stored statement: a replayed old one is not taken.
  const old = signed(9, doc(2, [2], [1]))
  assert.throws(() => fresh().accept(...old, NOW), /this release/)
  assert.equal(fresh().accept(...signed(9, doc(3, [2], [1])), NOW), 'updated')
  // A key the release dropped stays out of every statement.
  const withDropped = TrustState.load(null, root(9), { floor: { ...floor, revokedBundleKeys: [testKey(1).id] } })
  assert.throws(() => withDropped.accept(...signed(9, doc(4, [1], [])), NOW), /allowed again/)
  // Stored below the floor (an older release's file): ignored.
  const d = dir()
  state(9, d).accept(...old, NOW)
  assert.equal(fresh(d).statement(), null)
})

test('minisign key ids are read from keys and signatures', () => {
  const pair = minisignKey(3)
  const sig = minisignSignature(pair, Buffer.from('installer'), '1.0.0')
  assert.equal(minisignKeyId(pair.pubkey), pair.idHex)
  assert.equal(signatureKeyId(sig), pair.idHex)
  assert.equal(minisignKeyId('%%%'), null)
  assert.equal(signatureKeyId(''), null)
})

function updaterStatement(allowed: string[], revoked: string[]): Statement {
  return {
    schema: 1, serial: 2, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z',
    bundle_keys: [testKey(1).hex], revoked_bundle_keys: [], updater_keys: allowed, revoked_updater_keys: revoked, max_version: {},
  }
}

test('updates are checked against the updater keys of the statement', () => {
  const oldPair = minisignKey(3)
  const newPair = minisignKey(4)
  const oldSig = minisignSignature(oldPair, Buffer.from('installer'), '1.0.0')
  const newSig = minisignSignature(newPair, Buffer.from('installer'), '1.0.0')
  const choose = chooseUpdaterKey
  // No statement: the pinned key decides, whatever the signature says.
  assert.deepEqual(choose(oldPair.pubkey, null, newSig), { ok: true, key: oldPair.pubkey })
  // Listed and not revoked: accepted. A statement can introduce a key.
  const both = updaterStatement([oldPair.pubkey, newPair.pubkey], [])
  assert.deepEqual(choose(oldPair.pubkey, both, oldSig), { ok: true, key: oldPair.pubkey })
  assert.deepEqual(choose(oldPair.pubkey, both, newSig), { ok: true, key: newPair.pubkey })
  // Revoked: refused, even if it is the pinned one.
  const rotated = updaterStatement([newPair.pubkey], [oldPair.idHex])
  assert.deepEqual(choose(oldPair.pubkey, rotated, oldSig), { ok: false, refusal: 'revoked' })
  assert.deepEqual(choose(oldPair.pubkey, rotated, newSig), { ok: true, key: newPair.pubkey })
  // Not listed: refused. Malformed signature: refused.
  const onlyNew = updaterStatement([newPair.pubkey], [])
  assert.deepEqual(choose(oldPair.pubkey, onlyNew, oldSig), { ok: false, refusal: 'not_allowed' })
  assert.deepEqual(choose(oldPair.pubkey, onlyNew, 'garbage'), { ok: false, refusal: 'malformed' })
  // No updater keys listed: the pinned key stays, minus revocations.
  assert.deepEqual(choose(oldPair.pubkey, updaterStatement([], []), oldSig), { ok: true, key: oldPair.pubkey })
  assert.deepEqual(choose(oldPair.pubkey, updaterStatement([], [oldPair.idHex]), oldSig), { ok: false, refusal: 'revoked' })
})

test('times are parsed strictly to unix seconds', () => {
  assert.equal(parseUtc('1970-01-01T00:00:00Z'), 0)
  assert.equal(parseUtc('2026-11-01T00:00:00Z'), NOW)
  assert.equal(parseUtc('2024-02-29T12:30:45Z'), 1_709_209_845)
  assert.equal(parseUtc('2000-02-29T00:00:00Z'), 951_782_400)
  assert.notEqual(parseUtc('2026-04-30T00:00:00Z'), null)
  for (const bad of ['2026-11-01 00:00:00Z', '2026-11-01T00:00:00+00:00', '2026-13-01T00:00:00Z', '2026-11-32T00:00:00Z', '2026-11-01T24:00:00Z',
    '2026-11-01T00:60:00Z', '2026-11-01T00:00:60Z', '2026-1x-01T00:00:00Z', '1969-12-31T23:59:59Z', '',
    '2026-02-29T00:00:00Z', '2026-02-31T00:00:00Z', '2026-04-31T00:00:00Z', '2100-02-29T00:00:00Z', '2024-02-30T00:00:00Z']) {
    assert.equal(parseUtc(bad), null, bad)
  }
})

test('an expired statement is never accepted and a stored one stays in force but stale', () => {
  const [d, s] = signed(9, doc(1, [1], []))
  const after = parseUtc('2027-01-15T10:00:00Z') as number
  const trust = state(9)
  assert.equal(trust.fresh(NOW), false, 'no statement is not fresh')
  assert.throws(() => trust.accept(d, s, after), /expired/)
  assert.doesNotThrow(() => trust.accept(d, s, after - 1))
  assert.equal(trust.fresh(after - 1), true)
  // Past expiry: still the statement in force (its revocations count), but not fresh, and a replay of it is refused.
  assert.equal(trust.fresh(after), false)
  assert.equal(trust.statement()?.serial, 1)
  assert.throws(() => trust.accept(d, s, after))
  assert.equal(statementExpired(trust.statement() as Statement, after), true)
  assert.equal(state(9).fresh(NOW), false)
})

test('caps are part of the signed statement and must be well formed', () => {
  const withCaps = (max_version: StatementSpec['max_version']) => statementText({
    serial: 1, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z', max_version,
  })
  const trust = state(9)
  for (const bad of [{ x: '0.9.0' }, { 0: '0.9.0' }, { 1: 'latest' }] as Record<string, string>[]) {
    const [d, s] = signed(9, withCaps(bad))
    assert.throws(() => trust.accept(d, s, NOW))
  }
  const [d, s] = signed(9, withCaps({ 1: '0.9.0', 2: '1.0.0' }))
  trust.accept(d, s, NOW)
  const caps = statementCaps(trust.statement() as Statement)
  assert.deepEqual(caps.get(1), [0, 9, 0])
  assert.deepEqual(caps.get(2), [1, 0, 0])
  // Statements without the field have no caps.
  const [d2, s2] = signed(9, doc(2, [1], []))
  trust.accept(d2, s2, NOW)
  assert.equal(statementCaps(trust.statement() as Statement).size, 0)
})
