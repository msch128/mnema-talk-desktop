import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { PINNED_BUNDLE_KEYS, ROOT_PUB, UPDATER_PUBKEY } from '../src/core/pinned.ts'
import { parseKey } from '../src/core/bundle.ts'
import { type Statement, TrustState, chooseUpdaterKey, minisignKeyId, parseUtc, rootKey, signatureKeyId, statementBundleKeys, statementCaps, statementExpired, verifyStatement } from '../src/core/trust.ts'
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
  const stored = join(d, 'trust-state.json')
  const kept = readFileSync(stored)
  const edited = Buffer.from(docBytes)
  edited[5] ^= 1
  writeFileSync(stored, JSON.stringify({ statement: edited.toString('base64'), signature: sig.toString('base64') }))
  assert.equal(state(9, d).statement(), null)
  writeFileSync(stored, kept.subarray(0, 10))
  assert.equal(state(9, d).statement(), null)
})

test('a statement that cannot be stored is still in force, and an older pair of files is read', () => {
  const d = dir()
  const messages: string[] = []
  // A file where the directory should be: every write fails.
  const blocked = join(d, 'blocked')
  writeFileSync(blocked, 'x')
  const trust = TrustState.load(join(blocked, 'trust'), root(9), (m) => messages.push(m))
  const [docBytes, sig] = signed(9, doc(4, [2], [1]))
  assert.equal(trust.accept(docBytes, sig, NOW), 'updated')
  assert.equal(trust.statement()?.serial, 4, 'the revocation counts although the disk refused it')
  assert.equal(trust.effectiveKeys(trusted(1))[0]?.id, trusted(2)[0]?.id)
  assert.match(messages.join('\n'), /cannot store the trust statement/)
  // The pair of files an older version wrote still loads, and the next statement replaces it.
  const old = dir()
  writeFileSync(join(old, 'trust.json'), docBytes)
  writeFileSync(join(old, 'trust.json.sig'), sig)
  const migrated = state(9, old)
  assert.equal(migrated.statement()?.serial, 4)
  const [d5, s5] = signed(9, doc(5, [2], [1]))
  migrated.accept(d5, s5, NOW)
  assert.equal(existsSync(join(old, 'trust.json')), false)
  assert.equal(state(9, old).statement()?.serial, 5)
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
  for (const bad of ['2026-11-01 00:00:00Z', '2026-11-01T00:00:00+00:00', '2026-13-01T00:00:00Z', '2026-11-32T00:00:00Z', '2026-11-01T24:00:00Z',
    '2026-11-01T00:60:00Z', '2026-11-01T00:00:60Z', '2026-1x-01T00:00:00Z', '1969-12-31T23:59:59Z', '']) {
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
