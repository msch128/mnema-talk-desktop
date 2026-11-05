import { TrustState, chooseUpdaterKey, minisignKeyId, parseUtc, rootKey, signatureKeyId, statementBundleKeys, statementCaps, verifyStatement } from '../../src/core/trust.ts'
import { formatVersion } from '../../src/core/version.ts'
import { minisignKey, minisignSignature, signText, statementText, testKey, type StatementSpec } from '../../test/helpers/fixtures.ts'
import { hex, type Json } from './common.ts'

const NOW = 1_793_491_200 // 2026-11-01T00:00:00Z

const rootSeed = 9
const doc = (serial: number, allowed: number[], revoked: number[], extra: Partial<StatementSpec> = {}) => statementText({
  serial, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z',
  bundle_keys: allowed.map(hex), revoked_bundle_keys: revoked.map((s) => testKey(s).id), ...extra,
})
const signedBy = (seed: number, text: string) => ({ doc: text, sig: signText(testKey(seed), text) })
const pair = minisignKey(3)
const pair2 = minisignKey(4)

/** Statements, signed and not, with the time each is accepted at. */
function statementCases(): { name: string, doc: string, sig: string, now: number }[] {
  const base = doc(1, [1], [])
  const cases: { name: string, doc: string, sig: string, now: number }[] = []
  const add = (name: string, text: string, signer = rootSeed, now = NOW) => cases.push({ name, ...signedBy(signer, text), now })
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
  cases.push({ name: 'a signature that is not base64', doc: base, sig: '***', now: NOW })
  cases.push({ name: 'an empty signature', doc: base, sig: '', now: NOW })
  cases.push({ name: 'a signature of the wrong length', doc: base, sig: 'AAAA', now: NOW })
  cases.push({ name: 'a tampered document', doc: base.replace('2026-10-17', '2026-10-18'), sig: signText(testKey(rootSeed), base), now: NOW })
  cases.push({ name: 'a signature with whitespace around it', doc: base, sig: ` ${signText(testKey(rootSeed), base)}\n`, now: NOW })
  return cases
}

/** What `verifyStatement` and a fresh `TrustState` answer for each case. */
function statementResults(cases: ReturnType<typeof statementCases>, root: ReturnType<typeof rootKey>): Json[] {
  return cases.map((c) => {
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
}

/** Statements fed to one `TrustState` in turn: serial order, revocations that stay revoked. */
function sequenceResults(root: ReturnType<typeof rootKey>): Json[] {
  return [
    {
      name: 'an older serial is refused and an equal one changes nothing',
      steps: [signedBy(rootSeed, doc(2, [2], [1])), signedBy(rootSeed, doc(1, [1], [])), signedBy(rootSeed, doc(2, [2], [1])), signedBy(rootSeed, doc(2, [3], [])), signedBy(rootSeed, doc(3, [3], [1]))],
    },
    {
      name: 'a later statement cannot allow a revoked key again',
      steps: [signedBy(rootSeed, doc(2, [2], [1])), signedBy(rootSeed, doc(3, [1, 2], [])), signedBy(rootSeed, doc(4, [2, 3], []))],
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
}

/** Timestamps `parseUtc` reads or refuses. */
function timeVectors(): Json[] {
  return ['1970-01-01T00:00:00Z', '2026-11-01T00:00:00Z', '2024-02-29T12:30:45Z', '2000-01-01T00:00:00Z', '2026-11-01 00:00:00Z', '2026-11-01T00:00:00+00:00', '2026-13-01T00:00:00Z', '2026-11-32T00:00:00Z',
    '2026-11-01T24:00:00Z', '2026-11-01T00:60:00Z', '2026-11-01T00:00:60Z', '2026-1x-01T00:00:00Z', '1969-12-31T23:59:59Z', '2026-00-10T00:00:00Z', '2026-01-00T00:00:00Z',
    '2026-02-29T00:00:00Z', '2026-02-31T00:00:00Z', '2026-04-31T00:00:00Z', '2100-02-29T00:00:00Z', '2024-02-30T00:00:00Z', '2000-02-29T00:00:00Z', '2026-04-30T00:00:00Z', '']
    .map((input) => ({ input, secs: parseUtc(input) }))
}

function sigOf(m: ReturnType<typeof minisignKey>): string {
  return minisignSignature(m, Buffer.from('installer'), '1.0.0')
}

/** The key ids read out of minisign public keys and signatures. */
function minisignIds(): Json[] {
  return [
    { kind: 'key', input: pair.pubkey }, { kind: 'key', input: pair2.pubkey }, { kind: 'key', input: '%%%' }, { kind: 'key', input: '' },
    { kind: 'sig', input: sigOf(pair) }, { kind: 'sig', input: sigOf(pair2) }, { kind: 'sig', input: 'garbage' }, { kind: 'sig', input: '' },
    { kind: 'sig', input: minisignSignature(pair, Buffer.from('x'), '1.0.0', { prehashed: false }) },
  ].map((c) => ({ ...c, id: c.kind === 'key' ? minisignKeyId(c.input) : signatureKeyId(c.input) }))
}

/** Which updater key a statement lets sign an update. */
function updaterChoices(): Json[] {
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
  return choices.map((c) => {
    const full = c.statement && { schema: 1, serial: 2, issued_at: 'x', expires_at: 'y', bundle_keys: [], revoked_bundle_keys: [], max_version: {}, ...c.statement }
    const r = chooseUpdaterKey(c.pinned, full, c.signature)
    return { ...c, expect: r.ok ? { key: r.key } : { refusal: r.refusal } }
  })
}

export function trustVectors(): Json {
  const rootPub = hex(rootSeed)
  const root = rootKey(rootPub)
  const statements = statementResults(statementCases(), root)
  return { root: rootPub, now: NOW, times: timeVectors(), statements, sequences: sequenceResults(root), minisign_ids: minisignIds(), updater_choices: updaterChoices() }
}
