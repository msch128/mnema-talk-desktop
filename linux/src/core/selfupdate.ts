/**
 * Signed self-update of the desktop client itself. The web app has its own
 * signed bundles (`updates.ts`); this replaces the installed program. It
 * follows `experimental/app/src/native_updates.rs` and keeps its guarantees:
 *
 * Where bytes come from decides nothing. The release feed (`latest.json`, the
 * format of the Tauri updater) names a package and a minisign signature; the
 * package is installed only after the signature verifies against the public
 * key pinned in this build (or a replacement the trust statement lists), and
 * the version recorded inside the signature equals the announced one, so a
 * replayed older package cannot be paired with a newer version number. Only
 * versions greater than the running one are offered.
 *
 * "On next start" keeps the verified package on disk. It is verified again
 * against a freshly fetched feed before it runs.
 *
 * The program is the AppImage: installing replaces that one file. A `.deb`
 * install is updated through the package manager (`debinstall.ts`) from its own
 * feed entry (`linux-<arch>-deb`); other installations only get the release page.
 */
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { verifyPackage } from './minisign.ts'
import { CHECK_INTERVAL_SECS } from './store.ts'
import { type Statement, chooseUpdaterKey } from './trust.ts'
import { type FetchOptions, fetchLimited, updatesDisabled } from './updates.ts'
import { type Version, compareVersions, parseVersion } from './version.ts'

export const POLL_MS = 15 * 60 * 1000
export const CHECK_TIMEOUT_MS = 30_000
export const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000
export const START_TIMEOUT_MS = 8_000
const NOTES_MAX = 2000
const MAX_FEED = 1 << 20
const MAX_PACKAGE = 512 << 20
const STATE_FILE = 'native-update.json'
const PENDING_FILE = 'native-update.pending'

export type Phase = 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'installing' | 'failed'
export type Mode = 'now' | 'later'
export type Failure = 'offline' | 'verify' | 'install' | 'unknown'

/** What the update window shows. No raw error text crosses to the page. */
export interface Snapshot {
  current: string
  phase: Phase
  version: string | null
  notes: string | null
  received: number
  total: number | null
  mode: Mode | null
  error: Failure | null
  /** The package manager owns this installation: the window points to the download page instead of installing. */
  external: boolean
}

// ---------------------------------------------------------------- pure logic

/** Whether the interval has passed. A clock set back counts as due. */
export function checkDue(lastCheck: number, now: number): boolean {
  return lastCheck > now || now - lastCheck >= CHECK_INTERVAL_SECS
}

/** The candidate version when it is strictly newer than `current`. Versions are plain `MAJOR.MINOR.PATCH`; anything else is not offered. */
export function offerable(current: string, candidate: string): Version | null {
  const next = parseVersion(candidate)
  const now = parseVersion(current)
  return next && now && compareVersions(next, now) > 0 ? next : null
}

/** A skipped version stays quiet; a newer one is offered again. */
export function isSkipped(skipped: string | null, candidate: string): boolean {
  return skipped === candidate
}

/**
 * The package is fetched from GitHub's release downloads over HTTPS (which
 * redirects to GitHub's asset hosts). The signature decides trust; this keeps
 * the request from going elsewhere in the first place.
 */
export function downloadUrlOk(url: URL): boolean {
  return url.protocol === 'https:' && url.hostname === 'github.com' && url.username === '' && url.password === '' && url.port === ''
}

/** The `{os}-{arch}` key of the feed for this machine. */
export function platformKey(platform: string = process.platform, arch: string = process.arch, deb = false): string | null {
  if (platform !== 'linux') return null
  const suffix = deb ? '-deb' : ''
  if (arch === 'x64') return `linux-x86_64${suffix}`
  if (arch === 'arm64') return `linux-aarch64${suffix}`
  return null
}

export interface Update {
  version: string
  notes: string | null
  signature: string
  url: URL
  /** The minisign public key this update must verify against (pinned, or listed by the trust statement). */
  key: string
}

/** Reads the feed. Tauri clients and this one share it; entries for other platforms are ignored. */
export function parseFeed(bytes: Uint8Array, platform: string): Omit<Update, 'key'> | null {
  let feed: unknown
  try {
    feed = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
  } catch {
    return null
  }
  if (typeof feed !== 'object' || feed === null) return null
  const { version, notes, platforms } = feed as { version?: unknown, notes?: unknown, platforms?: unknown }
  if (typeof version !== 'string' || typeof platforms !== 'object' || platforms === null) return null
  const entry = (platforms as Record<string, unknown>)[platform]
  if (typeof entry !== 'object' || entry === null) return null
  const { signature, url } = entry as { signature?: unknown, url?: unknown }
  if (typeof signature !== 'string' || typeof url !== 'string') return null
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return null
  }
  return { version: version.replace(/^v/, ''), notes: trimNotes(typeof notes === 'string' ? notes : null), signature, url: parsed }
}

export function trimNotes(body: string | null): string | null {
  if (body === null) return null
  const kept = [...body].filter((c) => !/\p{Cc}/u.test(c) || c === '\n').slice(0, NOTES_MAX).join('')
  return kept.trim() === '' ? null : kept
}

// ------------------------------------------------------------------- state

interface Pending { version: string, signature: string }

interface Persisted {
  last_check: number
  skipped: string | null
  pending: Pending | null
}

function loadPersisted(dir: string | null): Persisted {
  const empty: Persisted = { last_check: 0, skipped: null, pending: null }
  if (!dir) return empty
  try {
    const raw: unknown = JSON.parse(readFileSync(join(dir, STATE_FILE), 'utf8'))
    if (typeof raw !== 'object' || raw === null) return empty
    const { last_check: last, skipped, pending } = raw as Record<string, unknown>
    const held = typeof pending === 'object' && pending !== null ? (pending as Record<string, unknown>) : null
    return {
      last_check: typeof last === 'number' && Number.isInteger(last) && last >= 0 ? last : 0,
      skipped: typeof skipped === 'string' ? skipped : null,
      pending: held && typeof held.version === 'string' && typeof held.signature === 'string' ? { version: held.version, signature: held.signature } : null,
    }
  } catch {
    return empty
  }
}

function writeAtomic(path: string, bytes: Uint8Array): void {
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 })
  const tmp = `${path}.tmp`
  writeFileSync(tmp, bytes, { mode: 0o600 })
  renameSync(tmp, path)
}

/** A dot marks a waiting update; the word is the same in every language. */
export function menuText(phase: Phase): string {
  return phase === 'available' || phase === 'ready' || phase === 'failed' ? 'Update •' : 'Updates'
}

/** What the updater needs from its surroundings. */
export interface UpdaterEnv {
  /** Version of the running program. */
  current: string
  /** Application data directory; null keeps everything in memory. */
  dir: string | null
  endpoints: readonly URL[]
  /** The pinned minisign public key. */
  pubkey: string
  /** The platform key of the feed, or null where there is no package for this machine. */
  platform: string | null
  /** The trust statement in force. */
  statement(): Statement | null
  /** A fresh trust statement is in force and this round's fetch succeeded. */
  installsAllowed(): boolean
  /** Fetches (a fresh) trust statement; resolves when the attempt is over. */
  trustRound?(): Promise<unknown>
  /** Replaces the installed program with the verified package. */
  install?(bytes: Buffer, version: string): Promise<void | 'handoff'>
  /** Asks before a kept package is installed at start (the installer needs the user's password). */
  confirmStart?(version: string): Promise<boolean>
  /** Starts the new version and quits this one. */
  restart?(): void
  now(): number
  /** Production only downloads from github.com; tests use a loopback server. */
  githubOnly: boolean
  fetchOptions?: FetchOptions
  /** The package manager owns this installation. */
  external: boolean
  onChange?(snapshot: Snapshot): void
  /** An update to announce (a new version, not skipped, found by a background check). */
  announce?(snapshot: Snapshot): void
  disabled?(): boolean
}

export class SelfUpdater {
  private persisted: Persisted
  private snap: Snapshot
  private update: Update | null = null
  private busy = false
  private readonly env: UpdaterEnv

  constructor(env: UpdaterEnv) {
    this.env = env
    this.persisted = loadPersisted(env.dir)
    // Already installed or superseded: nothing to wait for.
    if (this.persisted.pending && offerable(env.current, this.persisted.pending.version) === null) {
      this.persisted.pending = null
      if (env.dir) rmSync(join(env.dir, PENDING_FILE), { force: true })
      this.save()
    }
    this.snap = {
      current: env.current,
      phase: this.persisted.pending ? 'ready' : 'idle',
      version: this.persisted.pending?.version ?? null,
      notes: null,
      received: 0,
      total: null,
      mode: null,
      error: null,
      external: env.external,
    }
  }

  snapshot(): Snapshot {
    return { ...this.snap }
  }

  get lastCheck(): number {
    return this.persisted.last_check
  }

  hasPending(): boolean {
    return this.persisted.pending !== null
  }

  private save(): void {
    if (!this.env.dir) return
    try { writeAtomic(join(this.env.dir, STATE_FILE), Buffer.from(JSON.stringify(this.persisted))) } catch { /* kept in memory */ }
  }

  private change(f: () => void): void {
    f()
    this.env.onChange?.(this.snapshot())
  }

  private pendingPath(): string | null {
    return this.env.dir ? join(this.env.dir, PENDING_FILE) : null
  }

  // ---------------------------------------------------------- operations

  /** The feed, and the key the trust statement allows for the announced signature. */
  async fetch(): Promise<Update | null> {
    const platform = this.env.platform
    if (platform === null) return null
    let feedBytes: Buffer | null = null
    let failure: unknown = null
    for (const endpoint of this.env.endpoints) {
      try {
        feedBytes = await fetchLimited(endpoint, 'application/json', MAX_FEED, { ...this.env.fetchOptions, timeoutMs: CHECK_TIMEOUT_MS })
        break
      } catch (error) {
        failure = error
      }
    }
    if (!feedBytes) throw failure ?? new FailureError('offline')
    const found = parseFeed(feedBytes, platform)
    if (!found || offerable(this.env.current, found.version) === null) return null
    // The trust statement decides which updater key an update may be signed
    // with: a revoked key is refused, a listed replacement key is used.
    const chosen = chooseUpdaterKey(this.env.pubkey, this.env.statement(), found.signature)
    if (!chosen.ok) throw new FailureError('verify')
    if (this.env.githubOnly && !downloadUrlOk(found.url)) return null
    return { ...found, key: chosen.key }
  }

  /** Look for a newer version. `manual` shows the outcome (also failures); a background check stays silent unless it finds something new. */
  async check(manual: boolean): Promise<void> {
    if (await this.look(manual)) this.env.announce?.(this.snapshot())
  }

  /** The check itself; true when a new, not skipped version should be announced. */
  private async look(manual: boolean): Promise<boolean> {
    if (this.busy) return false
    this.busy = true
    if (manual) this.change(() => { this.snap.phase = 'checking'; this.snap.error = null })
    let found: Update | null = null
    let failure: Failure | null = null
    try {
      found = await this.fetch()
    } catch (error) {
      failure = classify(error)
    }
    let announce = false
    this.change(() => {
      this.busy = false
      this.persisted.last_check = this.env.now()
      if (failure) {
        if (manual) { this.snap.phase = 'failed'; this.snap.error = failure } else if (this.snap.phase === 'checking') this.snap.phase = 'idle'
      } else if (found) {
        const version = found.version
        const ready = this.persisted.pending?.version === version
        this.snap.version = version
        this.snap.notes = found.notes
        this.snap.received = 0
        this.snap.total = null
        this.snap.mode = null
        this.snap.error = null
        this.snap.phase = ready ? 'ready' : 'available'
        announce = !ready && !isSkipped(this.persisted.skipped, version)
        this.update = found
      } else {
        // Nothing newer. A held package for a version that is no longer offered is stale.
        this.update = null
        if (this.persisted.pending) {
          this.persisted.pending = null
          const path = this.pendingPath()
          if (path) rmSync(path, { force: true })
        }
        this.snap.version = null
        this.snap.notes = null
        this.snap.phase = manual ? 'up_to_date' : 'idle'
      }
      this.save()
    })
    return announce
  }

  /** Download, verify and either install at once (`now`) or keep the verified package for the next start (`later`). */
  async install(mode: Mode): Promise<void> {
    // After a restart a held package has no feed entry in memory yet.
    if (this.update === null && !this.busy) await this.check(false)
    if (this.busy || this.update === null) return
    const update = this.update
    this.busy = true
    this.change(() => {
      this.snap.phase = 'downloading'
      this.snap.mode = mode
      this.snap.received = 0
      this.snap.total = null
      this.snap.error = null
    })
    // The statement may have changed since the check. Without a fresh one
    // (expired, or its fetch failed this round) nothing new is installed.
    if (!this.env.installsAllowed() || !chooseUpdaterKey(this.env.pubkey, this.env.statement(), update.signature).ok) {
      return this.fail('verify')
    }
    let bytes = this.heldPackage(update)
    if (!bytes) {
      try {
        bytes = await fetchLimited(update.url, 'application/octet-stream', MAX_PACKAGE, {
          ...this.env.fetchOptions,
          timeoutMs: DOWNLOAD_TIMEOUT_MS,
          progress: (received, total) => this.change(() => { this.snap.received = received; this.snap.total = total }),
        })
      } catch (error) {
        return this.fail(classify(error))
      }
      // The signature decides, whatever the download said.
      if (verifyPackage(bytes, update.signature, update.key, update.version) !== null) return this.fail('verify')
    }
    if (mode === 'later') {
      const path = this.pendingPath()
      let stored = false
      if (path) {
        try { writeAtomic(path, bytes); stored = true } catch { stored = false }
      }
      if (!stored) return this.fail('install')
      this.change(() => {
        this.busy = false
        this.persisted.pending = { version: update.version, signature: update.signature }
        this.save()
        this.snap.phase = 'ready'
      })
      return
    }
    this.change(() => { this.snap.phase = 'installing' })
    try {
      if (!this.env.install) throw new FailureError('install')
      if ((await this.env.install(bytes, update.version)) === 'handoff') {
        // The system's installer finishes the job; nothing to restart here.
        this.change(() => { this.busy = false; this.snap.phase = 'available'; this.snap.mode = null })
        return
      }
      this.env.restart?.()
    } catch {
      this.fail('install')
    }
  }

  /** The package kept for the next start, if it is this very update and still verifies; saves downloading it a second time. */
  private heldPackage(update: Update): Buffer | null {
    const held = this.persisted.pending
    const path = this.pendingPath()
    if (!held || held.version !== update.version || !path) return null
    try {
      const bytes = readFileSync(path)
      const chosen = chooseUpdaterKey(this.env.pubkey, this.env.statement(), update.signature)
      return chosen.ok && verifyPackage(bytes, update.signature, chosen.key, update.version) === null ? bytes : null
    } catch {
      return null
    }
  }

  private fail(failure: Failure): void {
    this.change(() => {
      this.busy = false
      this.snap.phase = 'failed'
      this.snap.error = failure
    })
  }

  skip(): void {
    if (this.busy) return
    this.change(() => {
      if (this.snap.version) {
        this.persisted.skipped = this.snap.version
        this.save()
      }
      this.update = null
      this.snap.phase = 'idle'
      this.snap.version = null
      this.snap.notes = null
      this.snap.error = null
    })
  }

  discardPending(): void {
    this.change(() => {
      this.persisted.pending = null
      this.save()
      if (this.snap.phase === 'ready') {
        this.snap.phase = 'idle'
        this.snap.version = null
      }
    })
    const path = this.pendingPath()
    if (path) rmSync(path, { force: true })
  }

  /**
   * A package kept for this start, checked against a freshly fetched feed: it
   * must be the newest release and still verify. Bounded in time so that start
   * is never held up by the network.
   */
  async preparePending(): Promise<{ update: Update, bytes: Buffer } | null> {
    // Kept for later when no fresh trust statement is in force.
    if (!this.env.installsAllowed()) return null
    const pending = this.persisted.pending
    if (!pending) return null
    let update: Update | null
    try {
      update = await withTimeout(this.fetch(), START_TIMEOUT_MS)
    } catch {
      // Offline: keep it for the next start.
      return null
    }
    // Newer release or none: the held package is stale.
    if (!update || update.version !== pending.version) {
      this.discardPending()
      return null
    }
    const path = this.pendingPath()
    let bytes: Buffer | null
    try { bytes = path ? readFileSync(path) : null } catch { bytes = null }
    const verified = bytes !== null && offerable(this.env.current, update.version) !== null
      && verifyPackage(bytes, update.signature, update.key, update.version) === null
    if (!verified || !bytes) {
      this.discardPending()
      return null
    }
    return { update, bytes }
  }

  /** Installs a package kept for this start. Returns whether an install was started. */
  async applyPending(): Promise<boolean> {
    if (this.env.disabled?.() ?? updatesDisabled()) return false
    // The package is only run on the strength of a trust statement fetched in
    // this process: fetch it first (the main window is not up yet). If that
    // fails the package stays kept and the next start tries again.
    if (this.persisted.pending && !this.env.installsAllowed() && this.env.trustRound) {
      await withTimeout(this.env.trustRound(), START_TIMEOUT_MS).catch(() => undefined)
    }
    const prepared = await this.preparePending()
    if (!prepared) return false
    if (this.env.confirmStart && !(await this.env.confirmStart(prepared.update.version))) return false
    this.change(() => { this.snap.phase = 'installing'; this.snap.version = prepared.update.version })
    try {
      if (!this.env.install) throw new FailureError('install')
      if ((await this.env.install(prepared.bytes, prepared.update.version)) === 'handoff') {
        this.discardPending()
        return true
      }
      this.env.restart?.()
    } catch {
      this.discardPending()
      this.fail('install')
    }
    return true
  }

  /** Whether a scheduled check is due. */
  due(): boolean {
    return checkDue(this.persisted.last_check, this.env.now())
  }

  isBusyOrWaiting(): boolean {
    return ['available', 'ready', 'downloading', 'installing'].includes(this.snap.phase)
  }
}

class FailureError extends Error {
  readonly failure: Failure

  constructor(failure: Failure) {
    super(failure)
    this.failure = failure
  }
}

function classify(error: unknown): Failure {
  if (error instanceof FailureError) return error.failure
  return 'offline'
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timeout')), ms)
    promise.then((value) => { clearTimeout(timer); resolve(value) }, (error: unknown) => { clearTimeout(timer); reject(error instanceof Error ? error : new Error(String(error))) })
  })
}

// ------------------------------------------------------------ installation

/**
 * Replaces the running AppImage with a verified package: the new file is
 * written next to it and renamed over it, so a failure leaves the installed
 * program untouched. A running AppImage keeps working after the rename (its
 * inode stays mapped until it exits).
 */
export function replaceAppImage(target: string, bytes: Uint8Array): void {
  if (bytes.length < 4 || bytes[0] !== 0x7f || bytes[1] !== 0x45 || bytes[2] !== 0x4c || bytes[3] !== 0x46) throw new Error('not an executable')
  if (!existsSync(target)) throw new Error('the AppImage is not where it was started from')
  const fresh = `${target}.new`
  try {
    writeFileSync(fresh, bytes, { mode: 0o755 })
    chmodSync(fresh, 0o755)
    renameSync(fresh, target)
  } catch (error) {
    rmSync(fresh, { force: true })
    throw error
  }
}
