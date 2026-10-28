/**
 * Signed trust statement: remote revocation of the CI signing keys
 * (mnema-talk `doc/desktop-trusted-client.md`, "Key revocation"). A port of
 * `experimental/app/src/web_trust.rs`; the shared vectors must pass in both.
 *
 * The client pins one offline root key. A statement signed by it names the web
 * bundle keys and updater keys that are allowed and the ones that are revoked.
 * The signature is checked before anything is parsed, and a statement with a
 * lower serial than the highest one accepted is refused.
 */
import type { KeyObject } from 'node:crypto'
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeSync } from 'node:fs'
import { join } from 'node:path'
import { type TrustedKey, parseKey } from './bundle.ts'
import { publicKeyFromRaw, verifyEd25519 } from './ed25519.ts'
import { isLowerHex, lossyTrimmed, strictBase64 } from './encoding.ts'
import { type Version, parseVersion } from './version.ts'

export const MAX_STATEMENT = 64 << 10

/** The pair an older client wrote; read once, then replaced by the state file. */
const STATEMENT_FILE = 'trust.json'
const SIGNATURE_FILE = 'trust.json.sig'
/** Statement, signature and the revocations carried forward, in one file so a crash can never leave a new signature next to an old document. */
const STATE_FILE = 'trust-state.json'
/** Longest validity a statement may claim (the signing tool's bound). */
const MAX_VALIDITY_SECS = 400 * 86400

export interface Statement {
  schema: number
  serial: number
  issued_at: string
  expires_at: string
  bundle_keys: string[]
  revoked_bundle_keys: string[]
  updater_keys: string[]
  revoked_updater_keys: string[]
  /** Optional caps on the rollback marks, per web client API number. */
  max_version: Record<string, string>
}

const FIELDS = ['schema', 'serial', 'issued_at', 'expires_at', 'bundle_keys', 'revoked_bundle_keys', 'updater_keys', 'revoked_updater_keys', 'max_version']

export class TrustError extends Error {}

function fail(message: string): never {
  throw new TrustError(message)
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28
  return [4, 6, 9, 11].includes(month) ? 30 : 31
}

/** Seconds since the epoch for `YYYY-MM-DDTHH:MM:SSZ` (a real calendar date), nothing else. */
export function parseUtc(text: string): number | null {
  if (text.length !== 20 || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(text)) return null
  const num = (from: number, to: number) => Number(text.slice(from, to))
  const [y0, m, d] = [num(0, 4), num(5, 7), num(8, 10)]
  const [hh, mm, ss] = [num(11, 13), num(14, 16), num(17, 19)]
  if (m < 1 || m > 12 || d < 1 || d > daysInMonth(y0, m) || hh > 23 || mm > 59 || ss > 59) return null
  // Days since 1970-01-01 (Howard Hinnant's civil-to-days), as the Rust host does.
  const y = m <= 2 ? y0 - 1 : y0
  const era = Math.floor(y / 400)
  const yoe = y - era * 400
  const doy = Math.floor((153 * (m > 2 ? m - 3 : m + 9) + 2) / 5) + d - 1
  const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy
  const days = era * 146097 + doe - 719468
  const seconds = days * 86400 + hh * 3600 + mm * 60 + ss
  return seconds >= 0 ? seconds : null
}

function validId(id: string): boolean {
  return isLowerHex(id, 16)
}

function stringArray(value: unknown): string[] | null {
  return Array.isArray(value) && value.every((v) => typeof v === 'string') ? (value as string[]) : null
}

function parseStatement(doc: Uint8Array): Statement | null {
  let raw: unknown
  try {
    raw = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(doc))
  } catch {
    return null
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const record = raw as Record<string, unknown>
  const keys = Object.keys(record)
  if (!keys.every((k) => FIELDS.includes(k))) return null
  // Every field but `max_version` is required.
  if (!FIELDS.filter((f) => f !== 'max_version').every((f) => keys.includes(f))) return null
  const { schema, serial, issued_at, expires_at } = record
  if (typeof schema !== 'number' || !Number.isInteger(schema) || schema < 0 || schema > 0xffffffff) return null
  if (typeof serial !== 'number' || !Number.isSafeInteger(serial) || serial < 0) return null
  if (typeof issued_at !== 'string' || typeof expires_at !== 'string') return null
  const bundleKeys = stringArray(record.bundle_keys)
  const revokedBundleKeys = stringArray(record.revoked_bundle_keys)
  const updaterKeys = stringArray(record.updater_keys)
  const revokedUpdaterKeys = stringArray(record.revoked_updater_keys)
  if (!bundleKeys || !revokedBundleKeys || !updaterKeys || !revokedUpdaterKeys) return null
  let maxVersion: Record<string, string> = {}
  if (record.max_version !== undefined) {
    const cap = record.max_version
    if (typeof cap !== 'object' || cap === null || Array.isArray(cap)) return null
    if (!Object.values(cap).every((v) => typeof v === 'string')) return null
    maxVersion = cap as Record<string, string>
  }
  return { schema, serial, issued_at, expires_at, bundle_keys: bundleKeys, revoked_bundle_keys: revokedBundleKeys, updater_keys: updaterKeys, revoked_updater_keys: revokedUpdaterKeys, max_version: maxVersion }
}

/** Verifies the signature over the exact bytes, then parses and validates. */
export function verifyStatement(doc: Uint8Array, sig: Uint8Array, root: KeyObject): Statement {
  if (doc.length > MAX_STATEMENT) fail('trust statement too large')
  const signature = strictBase64(lossyTrimmed(sig))
  if (!signature || signature.length !== 64) fail('malformed trust signature')
  if (!verifyEd25519(root, doc, signature)) fail('trust signature does not match the root key')
  const statement = parseStatement(doc) ?? fail('unreadable trust statement')
  validate(statement)
  return statement
}

function validate(s: Statement): void {
  if (s.schema !== 1 || s.serial < 1) fail('unsupported trust statement')
  const issued = parseUtc(s.issued_at) ?? fail('invalid issued_at')
  const expires = parseUtc(s.expires_at) ?? fail('invalid expires_at')
  if (expires <= issued || expires - issued > MAX_VALIDITY_SECS) fail('invalid validity period')
  for (const [api, version] of Object.entries(s.max_version)) {
    const n = /^\+?[0-9]+$/.test(api) ? Number(api) : NaN
    if (!Number.isInteger(n) || n < 1 || n > 0xffffffff || parseVersion(version) === null) fail('invalid max_version')
  }
  if (s.bundle_keys.length === 0) fail('trust statement lists no bundle key')
  if (!s.revoked_bundle_keys.every(validId) || !s.revoked_updater_keys.every(validId)) fail('invalid revoked key id')
  for (const raw of s.bundle_keys) {
    const key = parseKey(raw) ?? fail('invalid bundle key')
    if (s.revoked_bundle_keys.includes(key.id)) fail('a bundle key is both allowed and revoked')
  }
  for (const key of s.updater_keys) {
    const id = minisignKeyId(key) ?? fail('invalid updater key')
    if (s.revoked_updater_keys.includes(id)) fail('an updater key is both allowed and revoked')
  }
}

/** The caps as numbers (validated when the statement was verified). */
export function statementCaps(s: Statement): Map<number, Version> {
  const caps = new Map<number, Version>()
  for (const [api, v] of Object.entries(s.max_version)) {
    const version = parseVersion(v)
    if (/^\+?[0-9]+$/.test(api) && version) caps.set(Number(api), version)
  }
  return caps
}

/** Whether the statement is past `expires_at` at `now` (unix seconds). */
export function statementExpired(s: Statement, now: number): boolean {
  const expires = parseUtc(s.expires_at)
  return expires === null || now >= expires
}

/** Allowed web bundle keys that are not revoked. */
export function statementBundleKeys(s: Statement): TrustedKey[] {
  return s.bundle_keys
    .map((raw) => parseKey(raw))
    .filter((k): k is TrustedKey => k !== null && !s.revoked_bundle_keys.includes(k.id))
}

function decodeText(wrapped: string): string | null {
  const raw = strictBase64(wrapped.trim())
  if (!raw) return null
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(raw)
  } catch {
    return null
  }
}

function secondLine(text: string | null): string | null {
  // Rust's `str::lines` splits on \n and drops one trailing \r per line.
  return text === null ? null : (text.split('\n')[1]?.replace(/\r$/, '') ?? null)
}

/** Key id (8 bytes, hex, file order) of a minisign public key in the form of `tauri.conf.json`: base64 of the `.pub` file. */
export function minisignKeyId(pubkey: string): string | null {
  const line = secondLine(decodeText(pubkey))
  const raw = line === null ? null : strictBase64(line.trim())
  return raw && raw.length === 42 && raw[0] === 0x45 && raw[1] === 0x64 ? raw.subarray(2, 10).toString('hex') : null
}

/** Key id named by a minisign signature (base64 of the `.sig` file). */
export function signatureKeyId(signature: string): string | null {
  const line = secondLine(decodeText(signature))
  const raw = line === null ? null : strictBase64(line.trim())
  return raw && raw.length === 74 && raw[0] === 0x45 && (raw[1] === 0x64 || raw[1] === 0x44) ? raw.subarray(2, 10).toString('hex') : null
}

export type UpdaterRefusal = 'revoked' | 'not_allowed' | 'malformed'

/**
 * The public key an update signed with `signature` must verify against.
 * Without a statement (or one that lists no updater key) that is the pinned
 * key. With one, the signature must name a listed, not revoked key, which may
 * replace the pinned one.
 */
export function chooseUpdaterKey(pinned: string, statement: Statement | null, signature: string): { ok: true, key: string } | { ok: false, refusal: UpdaterRefusal } {
  if (!statement) return { ok: true, key: pinned }
  const id = signatureKeyId(signature)
  if (id === null) return { ok: false, refusal: 'malformed' }
  if (statement.revoked_updater_keys.includes(id)) return { ok: false, refusal: 'revoked' }
  if (statement.updater_keys.length === 0) return { ok: true, key: pinned }
  const key = statement.updater_keys.find((k) => minisignKeyId(k) === id)
  return key === undefined ? { ok: false, refusal: 'not_allowed' } : { ok: true, key }
}

export type Accepted = 'updated' | 'unchanged'

/** What a release knows at build time, so a client with no stored statement (a fresh install, a removed file) cannot be talked back into an older state. */
export interface Floor {
  /** Statements with a lower serial are refused. */
  minSerial: number
  /** Key ids no statement may allow again. */
  revokedBundleKeys: readonly string[]
  revokedUpdaterKeys: readonly string[]
}

export const NO_FLOOR: Floor = { minSerial: 0, revokedBundleKeys: [], revokedUpdaterKeys: [] }

/**
 * The floor of this release: the serial of the statement it shipped with
 * (`experimental/app/fixtures/trust.json`). Raise it, and list the key ids
 * revoked since, when a later statement revokes a key.
 */
export const RELEASE_FLOOR: Floor = { minSerial: 3, revokedBundleKeys: [], revokedUpdaterKeys: [] }

export interface TrustOptions {
  floor?: Floor
  log?: (message: string) => void
}

interface Held { statement: Statement, doc: Buffer, sig: Buffer }

function writeAtomically(dir: string, name: string, bytes: Uint8Array): void {
  const partial = join(dir, `${name}.partial`)
  const fd = openSync(partial, 'w', 0o600)
  try {
    writeSync(fd, bytes)
    fsyncSync(fd)
  } finally {
    closeSync(fd)
  }
  renameSync(partial, join(dir, name))
}

/** The last good statement, persisted next to the bundle cache. */
export class TrustState {
  private current: Held | null = null
  /** Every key id any accepted statement (or the floor) revoked. A later statement can neither allow one again nor drop it from the lists in force. */
  private readonly revokedBundle: Set<string>
  private readonly revokedUpdater: Set<string>
  /** The state in force is on disk. */
  private stored = true
  private readonly floor: Floor
  private readonly log: (message: string) => void

  /** Loads and re-verifies the stored statement; a damaged or foreign file is ignored, which falls back to the compiled-in keys. */
  static load(dir: string | null, root: KeyObject, options: TrustOptions = {}): TrustState {
    const state = new TrustState(dir, root, options)
    if (dir) {
      try {
        const { doc, sig, bundle, updater } = readStored(dir)
        const statement = verifyStatement(doc, sig, root)
        // A statement below the floor is an older release's.
        if (statement.serial >= state.floor.minSerial) {
          bundle.forEach((id) => state.revokedBundle.add(id))
          updater.forEach((id) => state.revokedUpdater.add(id))
          state.hold(statement, doc, sig)
        }
      } catch { /* ignored: the compiled-in keys apply */ }
    }
    return state
  }

  private readonly dir: string | null
  private readonly root: KeyObject

  private constructor(dir: string | null, root: KeyObject, options: TrustOptions) {
    this.dir = dir
    this.root = root
    this.floor = options.floor ?? NO_FLOOR
    this.log = options.log ?? (() => {})
    this.revokedBundle = new Set(this.floor.revokedBundleKeys)
    this.revokedUpdater = new Set(this.floor.revokedUpdaterKeys)
  }

  /** Puts a verified statement in force, with every revocation seen so far. */
  private hold(statement: Statement, doc: Buffer, sig: Buffer): void {
    statement.revoked_bundle_keys.forEach((id) => this.revokedBundle.add(id))
    statement.revoked_updater_keys.forEach((id) => this.revokedUpdater.add(id))
    this.current = {
      statement: { ...statement, revoked_bundle_keys: [...this.revokedBundle].sort(), revoked_updater_keys: [...this.revokedUpdater].sort() },
      doc,
      sig,
    }
  }

  statement(): Statement | null {
    return this.current?.statement ?? null
  }

  /**
   * A statement is in force and has not expired. An expired one still decides
   * which keys count (its revocations stay), but nothing new is installed while
   * it is not fresh.
   */
  fresh(now: number): boolean {
    const s = this.statement()
    return s !== null && !statementExpired(s, now)
  }

  /** Bundle keys in force: the statement's, else the compiled-in ones. */
  effectiveKeys(pinned: TrustedKey[]): TrustedKey[] {
    const s = this.statement()
    return s ? statementBundleKeys(s) : pinned
  }

  /** Writes the state in force in one atomic step. */
  private persist(): void {
    if (!this.dir || !this.current) return
    mkdirSync(this.dir, { recursive: true, mode: 0o700 })
    const stored = {
      statement: this.current.doc.toString('base64'),
      signature: this.current.sig.toString('utf8'),
      revoked_bundle_keys: [...this.revokedBundle].sort(),
      revoked_updater_keys: [...this.revokedUpdater].sort(),
    }
    writeAtomically(this.dir, STATE_FILE, Buffer.from(JSON.stringify(stored)))
    // The pair an older client wrote must not outlive the state: a stale one would be read back should the state file ever be lost.
    for (const name of [STATEMENT_FILE, SIGNATURE_FILE]) rmSync(join(this.dir, name), { force: true })
  }

  private store(): void {
    try {
      this.persist()
      this.stored = true
    } catch {
      this.stored = false
      this.log('cannot store the trust statement; it applies until restart')
    }
  }

  /**
   * Takes a fetched statement. An expired one is never accepted, lower serials
   * are refused, and the same serial only with identical bytes. A key an
   * earlier statement revoked cannot be allowed again. The statement applies as
   * soon as it verified; failing to store it is logged and retried with the
   * next identical fetch.
   */
  accept(doc: Uint8Array, sig: Uint8Array, now: number): Accepted {
    const statement = verifyStatement(doc, sig, this.root)
    if (statementExpired(statement, now)) fail('trust statement has expired')
    if (statement.serial < this.floor.minSerial) fail(`trust statement ${statement.serial} is older than this release accepts`)
    if (this.current) {
      const held = this.current.statement
      if (statement.serial < held.serial) fail(`trust statement ${statement.serial} is older than the ${held.serial} already accepted`)
      if (statement.serial === held.serial) {
        if (Buffer.compare(this.current.doc, doc) !== 0) fail('a different trust statement with the same serial')
        if (!this.stored) this.store()
        return 'unchanged'
      }
    }
    if (statement.bundle_keys.some((raw) => { const key = parseKey(raw); return key !== null && this.revokedBundle.has(key.id) })) fail('a revoked bundle key is allowed again')
    if (statement.updater_keys.some((raw) => { const id = minisignKeyId(raw); return id !== null && this.revokedUpdater.has(id) })) fail('a revoked updater key is allowed again')
    this.hold(statement, Buffer.from(doc), Buffer.from(sig))
    this.store()
    return 'updated'
  }
}

/** The stored bytes: the state file, else the pair an older client wrote. */
function readStored(dir: string): { doc: Buffer, sig: Buffer, bundle: string[], updater: string[] } {
  let raw: Buffer | null = null
  try { raw = readFileSync(join(dir, STATE_FILE)) } catch { /* no state file yet */ }
  if (raw === null) return { doc: readFileSync(join(dir, STATEMENT_FILE)), sig: readFileSync(join(dir, SIGNATURE_FILE)), bundle: [], updater: [] }
  const stored = JSON.parse(raw.toString('utf8')) as Record<string, unknown>
  if (typeof stored.statement !== 'string' || typeof stored.signature !== 'string') throw new Error('unreadable trust state')
  const ids = (value: unknown): string[] => (Array.isArray(value) ? value.filter((id): id is string => typeof id === 'string' && validId(id)) : [])
  return { doc: Buffer.from(stored.statement, 'base64'), sig: Buffer.from(stored.signature, 'utf8'), bundle: ids(stored.revoked_bundle_keys), updater: ids(stored.revoked_updater_keys) }
}

export function rootKey(rawHex: string): KeyObject {
  const raw = Buffer.from(rawHex, 'hex')
  const key = raw.length === 32 ? publicKeyFromRaw(raw) : null
  if (!key) throw new Error('the pinned root key is not a valid Ed25519 key')
  return key
}
