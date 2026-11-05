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
import { readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { verifyPackage } from './minisign.ts'
import { chooseUpdaterKey } from './trust.ts'
import { fetchLimited, updatesDisabled } from './updates.ts'
import { type Update, checkDue, downloadUrlOk, isSkipped, offerable, parseFeed } from './updatefeed.ts'
import { PENDING_FILE, STATE_FILE, type Persisted, loadPersisted, writeAtomic } from './updaterstate.ts'
import { type Failure, type Mode, type Snapshot, type UpdaterEnv, FailureError, classify, withTimeout } from './updatertypes.ts'

export { checkDue, downloadUrlOk, isSkipped, offerable, parseFeed, platformKey, trimNotes, type Update } from './updatefeed.ts'
export { replaceAppImage } from './appimage.ts'
export { type Failure, type Mode, type Phase, type Snapshot, type UpdaterEnv, menuText } from './updatertypes.ts'

export const POLL_MS = 15 * 60 * 1000
const CHECK_TIMEOUT_MS = 30_000
const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000
const START_TIMEOUT_MS = 8_000
const MAX_FEED = 1 << 20
const MAX_PACKAGE = 512 << 20

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
      // A failed look (offline at start) does not use up the interval: the next poll tries again.
      if (!failure) this.persisted.last_check = this.env.now()
      if (failure) this.lookFailed(manual, failure)
      else if (found) announce = this.lookFound(found)
      else this.lookNothing(manual)
      this.save()
    })
    return announce
  }

  private lookFailed(manual: boolean, failure: Failure): void {
    if (manual) { this.snap.phase = 'failed'; this.snap.error = failure } else if (this.snap.phase === 'checking') this.snap.phase = 'idle'
  }

  /** A newer version is on offer; true when it should be announced (new, not skipped, not already kept). */
  private lookFound(found: Update): boolean {
    const version = found.version
    const ready = this.persisted.pending?.version === version
    this.snap.version = version
    this.snap.notes = found.notes
    this.snap.received = 0
    this.snap.total = null
    this.snap.mode = null
    this.snap.error = null
    this.snap.phase = ready ? 'ready' : 'available'
    this.update = found
    return !ready && !isSkipped(this.persisted.skipped, version)
  }

  private lookNothing(manual: boolean): void {
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
    const bytes = await this.obtainPackage(update)
    if (bytes === null) return
    if (mode === 'later') {
      this.keepForLater(update, bytes)
      return
    }
    await this.installNow(update, bytes)
  }

  /** The verified package of `update`, from disk or the network; null once the failure has been recorded. */
  private async obtainPackage(update: Update): Promise<Buffer | null> {
    // The statement may have changed since the check. Without a fresh one
    // (expired, or its fetch failed this round) nothing new is installed.
    if (!this.env.installsAllowed() && this.env.trustRound) {
      // Started offline, say: the statement of this round is fetched now instead of reporting the signature as wrong.
      await withTimeout(this.env.trustRound(), START_TIMEOUT_MS).catch(() => undefined)
    }
    if (!this.env.installsAllowed()) return this.failed('untrusted')
    if (!chooseUpdaterKey(this.env.pubkey, this.env.statement(), update.signature).ok) return this.failed('verify')
    const held = this.heldPackage(update)
    if (held) return held
    let bytes: Buffer
    try {
      bytes = await fetchLimited(update.url, 'application/octet-stream', MAX_PACKAGE, {
        ...this.env.fetchOptions,
        timeoutMs: DOWNLOAD_TIMEOUT_MS,
        progress: (received, total) => this.change(() => { this.snap.received = received; this.snap.total = total }),
      })
    } catch (error) {
      return this.failed(classify(error))
    }
    // The signature decides, whatever the download said.
    if (verifyPackage(bytes, update.signature, update.key, update.version) !== null) return this.failed('verify')
    return bytes
  }

  private failed(failure: Failure): null {
    this.fail(failure)
    return null
  }

  /** Keeps the verified package on disk for the next start. */
  private keepForLater(update: Update, bytes: Buffer): void {
    const path = this.pendingPath()
    let stored = false
    if (path) {
      try { writeAtomic(path, bytes); stored = true } catch { stored = false }
    }
    if (!stored) return this.fail('install')
    this.change(() => {
      this.busy = false
      this.persisted.pending = { version: update.version }
      this.save()
      this.snap.phase = 'ready'
    })
  }

  private async installNow(update: Update, bytes: Buffer): Promise<void> {
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
