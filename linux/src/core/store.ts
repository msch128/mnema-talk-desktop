/**
 * Verified bundles in memory plus the cache on disk, the rollback marks and the
 * selection against a server. A port of `Store` in
 * `experimental/app/src/web_bundle.rs`.
 */
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, unlinkSync } from 'node:fs'
import { join } from 'node:path'
import { writeFileAtomic } from './atomic.ts'
import { MAX_ZIP, type Bundle, type TrustedKey, BundleError, verifyBundle } from './bundle.ts'
import { type Version, ZERO, compareVersions, formatVersion, maxVersion, minVersion, parseVersion } from './version.ts'

const MAX_CACHED_FILES = 16
/** Cached bundles kept besides the built-in one. */
const KEEP_CACHED = 3
export const CHECK_INTERVAL_SECS = 6 * 60 * 60

type Source = 'builtin' | 'cache'

export interface Held {
  readonly bundle: Bundle
  readonly source: Source
}

/** What the instance said about itself in `/api/health`. */
export interface ServerInfo {
  /** `null` for anything that is not a release version, such as `dev`. */
  readonly version: Version | null
  readonly webClientApi: number
  /** What the server calls itself (`community_name` of `/api/health`); absent from an older server. */
  readonly communityName?: string | null
}

/** Why no held bundle fits a server. Never answered by running the server's own JavaScript. */
export type Refusal =
  | { kind: 'no_bundle' }
  | { kind: 'server_newer' }
  | { kind: 'server_too_old', needs: Version }
  | { kind: 'server_contract_retired' }
  | { kind: 'rolled_back' }
  | { kind: 'signer_withdrawn' }

export function refusalText(refusal: Refusal): string {
  switch (refusal.kind) {
    case 'no_bundle': return 'This client has no verified app. Reinstall the desktop client.'
    case 'server_newer': return 'This server needs a newer Mnema app than this client has. Update the desktop client.'
    case 'server_too_old': return `This server is too old for this desktop client (needs ${formatVersion(refusal.needs)} or newer). Ask the administrator to update it.`
    case 'server_contract_retired': return 'This server speaks an older contract than any app in this client. Ask the administrator to update it.'
    case 'signer_withdrawn': return 'The app in this client was signed with a key that is no longer trusted. Update the desktop client; it fetches a newly signed app at its next check.'
    case 'rolled_back': return 'The newest verified app this client has used is no longer available, and older apps are refused. Update or reinstall the desktop client.'
  }
}

export type SelectResult = { ok: true, held: Held } | { ok: false, refusal: Refusal }

interface PersistedState {
  /** Highest `security_floor` each signing key ever declared (key id). */
  floors: Record<string, string>
  /** Highest bundle version ever verified per `web_client_api` and signing key. */
  marks: Record<string, Record<string, string>>
  last_check: number
}

function readVersions(raw: unknown): Map<string, Version> {
  const out = new Map<string, Version>()
  if (typeof raw === 'object' && raw !== null) {
    for (const [key, value] of Object.entries(raw)) {
      const v = typeof value === 'string' ? parseVersion(value) : null
      if (v) out.set(key, v)
    }
  }
  return out
}

function verifyFile(path: string, keys: readonly TrustedKey[]): Bundle {
  let size: number
  try {
    size = statSync(path).size
  } catch {
    throw new BundleError('unreadable')
  }
  if (size > MAX_ZIP) throw new BundleError('unreadable')
  let bytes: Buffer
  try {
    bytes = readFileSync(path)
  } catch {
    throw new BundleError('unreadable')
  }
  return verifyBundle(bytes, keys)
}

function cachedFiles(dir: string | null): string[] {
  if (!dir) return []
  let names: string[]
  try {
    names = readdirSync(dir)
  } catch {
    return []
  }
  const files = names
    .filter((name) => name.startsWith('mnema-web-') && name.endsWith('.zip'))
    .filter((name) => { try { return statSync(join(dir, name)).isFile() } catch { return false } })
    .sort()
  return files.slice(0, MAX_CACHED_FILES)
}

export class Store {
  /** Names of cached files that failed verification (diagnostics only). */
  rejected: string[] = []
  private readonly dir: string | null
  private floorInForce: Version = ZERO
  private highWaterInForce = new Map<number, Version>()
  private floors: Map<string, Version>
  private marks: Map<number, Map<string, Version>>
  private trusted: string[]
  private lastCheck: number
  private withdrawn: boolean
  private heldBundles: Held[]
  /** Verified bundles below the floor. Never served; they return if the key that raised the floor is revoked. */
  private shelved: Held[] = []

  /**
   * Verifies the built-in bundle and everything in the cache directory. The
   * security floor is the highest floor any verified manifest declares (or an
   * earlier run recorded); bundles below it are dropped.
   */
  static load(dir: string | null, builtin: Uint8Array | null, keys: readonly TrustedKey[]): Store {
    let state: Partial<PersistedState> = {}
    if (dir) {
      try {
        const parsed: unknown = JSON.parse(readFileSync(join(dir, 'state.json'), 'utf8'))
        if (typeof parsed === 'object' && parsed !== null) state = parsed as Partial<PersistedState>
      } catch { /* a missing or damaged state is rebuilt from the held bundles */ }
    }
    const verified: Held[] = []
    const rejected: string[] = []
    if (builtin) {
      try {
        verified.push({ bundle: verifyBundle(builtin, keys), source: 'builtin' })
      } catch (error) {
        if (!(error instanceof BundleError)) throw error
        rejected.push('built-in')
      }
    }
    for (const name of cachedFiles(dir)) {
      try {
        const bundle = verifyFile(join(dir as string, name), keys)
        if (!verified.some((h) => compareVersions(h.bundle.version, bundle.version) === 0)) verified.push({ bundle, source: 'cache' })
      } catch (error) {
        if (!(error instanceof BundleError)) throw error
        rejected.push(name)
      }
    }
    const marks = new Map<number, Map<string, Version>>()
    for (const [api, by] of Object.entries(state.marks ?? {})) {
      const n = Number(api)
      if (Number.isInteger(n) && n >= 1) marks.set(n, readVersions(by))
    }
    const store = new Store(dir, verified, rejected, readVersions(state.floors), marks, keys.map((k) => k.id),
      typeof state.last_check === 'number' ? state.last_check : 0)
    store.raiseFromHeld()
    store.recompute()
    store.applyFloor()
    store.persist()
    return store
  }

  private constructor(dir: string | null, held: Held[], rejected: string[], floors: Map<string, Version>, marks: Map<number, Map<string, Version>>, trusted: string[], lastCheck: number) {
    this.dir = dir
    this.heldBundles = held
    this.rejected = rejected
    this.withdrawn = rejected.length > 0
    this.floors = floors
    this.marks = marks
    this.trusted = trusted
    this.lastCheck = lastCheck
  }

  private persist(): void {
    if (!this.dir) return
    const state: PersistedState = {
      floors: Object.fromEntries([...this.floors].map(([k, v]) => [k, formatVersion(v)])),
      marks: Object.fromEntries([...this.marks].map(([api, by]) => [String(api), Object.fromEntries([...by].map(([k, v]) => [k, formatVersion(v)]))])),
      last_check: this.lastCheck,
    }
    try {
      mkdirSync(this.dir, { recursive: true, mode: 0o700 })
      writeFileAtomic(join(this.dir, 'state.json'), Buffer.from(JSON.stringify(state)))
    } catch { /* the marks are rebuilt from the held bundles */ }
  }

  floor(): Version { return this.floorInForce }

  highWater(api: number): Version | undefined { return this.highWaterInForce.get(api) }

  /** Why nothing at all can be served. */
  emptyReason(): Refusal {
    return this.withdrawn ? { kind: 'signer_withdrawn' } : { kind: 'no_bundle' }
  }

  newest(): Held | undefined { return this.heldBundles[0] }

  held(): readonly Held[] { return this.heldBundles }

  checkDue(now: number): boolean {
    return Math.max(0, now - this.lastCheck) >= CHECK_INTERVAL_SECS
  }

  markChecked(now: number): void {
    this.lastCheck = now
    this.persist()
  }

  /**
   * The newest held bundle that speaks the server's contract and supports its
   * release. Bundles older than the high-water mark of their contract are never
   * offered: a server cannot talk the client into an older, vulnerable app by
   * claiming an old version.
   */
  select(server: ServerInfo): SelectResult {
    const current = this.heldBundles.filter((h) => {
      const mark = this.highWaterInForce.get(h.bundle.webClientApi)
      return mark === undefined || compareVersions(h.bundle.version, mark) >= 0
    })
    const speaks = (h: Held) => h.bundle.webClientApi === server.webClientApi
    const supports = (h: Held) => server.version === null || compareVersions(server.version, h.bundle.minServerVersion) >= 0
    const best = current.filter(speaks).find(supports)
    if (best) return { ok: true, held: best }
    if (this.heldBundles.length === 0) return { ok: false, refusal: this.emptyReason() }
    const newestApi = Math.max(...this.heldBundles.map((h) => h.bundle.webClientApi))
    if (server.webClientApi > newestApi) return { ok: false, refusal: { kind: 'server_newer' } }
    const minimums = current.filter(speaks).map((h) => h.bundle.minServerVersion)
    if (minimums.length > 0) {
      return { ok: false, refusal: { kind: 'server_too_old', needs: minimums.reduce(minVersion) } }
    }
    // Bundles for this contract are held but all below the newest this client
    // has accepted. If the newest one was dropped because its key was revoked,
    // say that instead.
    if (this.heldBundles.some(speaks)) {
      return { ok: false, refusal: { kind: this.withdrawn ? 'signer_withdrawn' : 'rolled_back' } }
    }
    return { ok: false, refusal: { kind: 'server_contract_retired' } }
  }

  /**
   * Verifies a downloaded bundle and keeps it if it is newer than everything
   * held. Older and equal versions are refused, so replayed release metadata
   * cannot replace anything.
   */
  install(bytes: Uint8Array, keys: readonly TrustedKey[]): Version {
    const bundle = verifyBundle(bytes, keys)
    const version = bundle.version
    // Per contract: a patch for an older web client API is not shadowed by a
    // newer bundle of another one. A version is held only once (it names the file).
    const held = (h: Held) =>
      compareVersions(h.bundle.version, version) === 0 ||
      (h.bundle.webClientApi === bundle.webClientApi && compareVersions(h.bundle.version, version) > 0)
    if (this.heldBundles.some(held)) {
      throw new BundleError('a bundle of this version or newer is already held')
    }
    if (compareVersions(version, maxVersion(this.floorInForce, bundle.securityFloor)) < 0) throw new BundleError('below the security floor')
    if (this.dir) {
      try {
        mkdirSync(this.dir, { recursive: true, mode: 0o700 })
      } catch {
        throw new BundleError('cannot create the cache directory')
      }
      try {
        writeFileAtomic(join(this.dir, `mnema-web-${formatVersion(version)}.zip`), bytes)
      } catch {
        throw new BundleError('cannot write the bundle')
      }
    }
    this.trusted = keys.map((k) => k.id)
    this.raiseMarks(bundle)
    this.recompute()
    this.heldBundles.push({ bundle, source: 'cache' })
    this.applyFloor()
    this.prune()
    this.persist()
    return version
  }

  /**
   * Applies a new set of trusted keys to what is held: bundles signed by any
   * other key are dropped (cached files removed), and the floor and marks are
   * recomputed from the keys still trusted. Returns whether anything was dropped.
   */
  reconcile(keys: readonly TrustedKey[]): boolean {
    this.trusted = keys.map((k) => k.id)
    const trusted = (id: string) => keys.some((k) => k.id === id)
    const dropped = this.heldBundles.filter((h) => !trusted(h.bundle.keyId)).map((h) => ({ version: h.bundle.version, cached: h.source === 'cache' }))
    this.heldBundles = this.heldBundles.filter((h) => trusted(h.bundle.keyId))
    this.shelved = this.shelved.filter((h) => trusted(h.bundle.keyId))
    if (this.dir) {
      for (const { version, cached } of dropped) {
        if (cached) this.removeCached(version)
      }
    }
    if (dropped.length > 0) this.withdrawn = true
    // The marks in force now come from still-trusted keys only.
    this.recompute()
    this.applyFloor()
    this.persist()
    return dropped.length > 0
  }

  /**
   * Applies the caps of a newly taken trust statement: the recorded marks per
   * web client API and the floors are lowered to at most the cap, and held
   * bundles above a cap are dropped. This clears a mark raised by a forged
   * bundle even when no trusted bundle exists yet.
   */
  applyCaps(caps: ReadonlyMap<number, Version>): void {
    if (caps.size === 0) return
    const highest = [...caps.values()].reduce(maxVersion)
    for (const [api, cap] of caps) {
      let by = this.marks.get(api)
      if (!by) { by = new Map(); this.marks.set(api, by) }
      for (const [key, mark] of by) by.set(key, minVersion(mark, cap))
    }
    for (const [key, floor] of this.floors) this.floors.set(key, minVersion(floor, highest))
    const over = (h: Held) => {
      const cap = caps.get(h.bundle.webClientApi)
      return cap !== undefined && compareVersions(h.bundle.version, cap) > 0
    }
    // Their cached files go too: left on disk, the next start would verify them again and raise the marks anew.
    for (const h of [...this.heldBundles, ...this.shelved]) if (over(h) && h.source === 'cache') this.removeCached(h.bundle.version)
    this.heldBundles = this.heldBundles.filter((h) => !over(h))
    this.shelved = this.shelved.filter((h) => !over(h))
    this.recompute()
    this.applyFloor()
    this.persist()
  }

  /**
   * Floor and high-water marks in force: the highest recorded by any key that
   * is trusted now. Revoking a key clears what it raised and nothing a trusted
   * key reached or declared.
   */
  private recompute(): void {
    const counts = (key: string) => this.trusted.includes(key)
    const floors = [...this.floors].filter(([k]) => counts(k)).map(([, v]) => v)
    this.floorInForce = floors.length > 0 ? floors.reduce(maxVersion) : ZERO
    this.highWaterInForce = new Map()
    for (const [api, by] of this.marks) {
      const versions = [...by].filter(([k]) => counts(k)).map(([, v]) => v)
      if (versions.length > 0) this.highWaterInForce.set(api, versions.reduce(maxVersion))
    }
  }

  private raiseFromHeld(): void {
    for (const h of [...this.heldBundles, ...this.shelved]) this.raiseMarks(h.bundle)
  }

  /** Serves bundles at or above the floor, newest first. */
  private applyFloor(): void {
    const all = [...this.heldBundles, ...this.shelved]
    this.heldBundles = all.filter((h) => compareVersions(h.bundle.version, this.floorInForce) >= 0)
    this.shelved = all.filter((h) => compareVersions(h.bundle.version, this.floorInForce) < 0)
    this.heldBundles.sort((a, b) => compareVersions(b.bundle.version, a.bundle.version))
  }

  private raiseMarks(bundle: Bundle): void {
    const floor = this.floors.get(bundle.keyId)
    this.floors.set(bundle.keyId, floor === undefined ? bundle.securityFloor : maxVersion(floor, bundle.securityFloor))
    let by = this.marks.get(bundle.webClientApi)
    if (!by) { by = new Map(); this.marks.set(bundle.webClientApi, by) }
    const mark = by.get(bundle.keyId)
    by.set(bundle.keyId, mark === undefined ? bundle.version : maxVersion(mark, bundle.version))
  }

  /**
   * Keeps the built-in bundle and, per web client API, the newest cached ones,
   * served or shelved; removes the files of the rest.
   */
  private prune(): void {
    const dropped: Version[] = []
    const keep = (list: Held[]): Held[] => {
      const cached = new Map<number, number>()
      return list.filter((h) => {
        if (h.source === 'builtin') return true
        const n = (cached.get(h.bundle.webClientApi) ?? 0) + 1
        cached.set(h.bundle.webClientApi, n)
        if (n > KEEP_CACHED) { dropped.push(h.bundle.version); return false }
        return true
      })
    }
    this.heldBundles = keep(this.heldBundles)
    this.shelved = keep(this.shelved)
    for (const version of dropped) this.removeCached(version)
  }

  private removeCached(version: Version): void {
    if (!this.dir) return
    const file = join(this.dir, `mnema-web-${formatVersion(version)}.zip`)
    try { if (existsSync(file)) unlinkSync(file) } catch { /* best effort */ }
  }
}

/**
 * Installs a bundle against the keys in force *now*. Installation is
 * synchronous, and the keys are read at the moment it runs, so a bundle cannot
 * be installed with keys that a trust statement has just withdrawn.
 */
export function installCurrent(store: Store, keys: () => readonly TrustedKey[], bytes: Uint8Array): Version {
  return store.install(bytes, keys())
}
