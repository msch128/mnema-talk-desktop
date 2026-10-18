/**
 * The state of the trusted client: verified bundles, the keys in force, the
 * trust statement, the selected instance and its sockets. A port of `Trusted`
 * in `experimental/app/src/web_client.rs`, without the windowing (see
 * `src/main`).
 */
import { createHash } from 'node:crypto'
import { join } from 'node:path'
import { type TrustedKey, parseKey } from './bundle.ts'
import { parseAddress } from './client.ts'
import { Jar, type Vault } from './cookies.ts'
import { discover } from './discover.ts'
import { Limits, type Instance } from './proxy.ts'
import { PINNED_BUNDLE_KEYS, ROOT_PUB } from './pinned.ts'
import { Sockets } from './sockets.ts'
import { compareVersions } from './version.ts'
import { type Held, type ServerInfo, Store, refusalText } from './store.ts'
import { TrustError, TrustState, type Statement, rootKey, statementCaps } from './trust.ts'
import { checkAndInstall as installNewer, fetchTrust as fetchStatement, updatesDisabled } from './updates.ts'

export function pinnedKeys(): TrustedKey[] {
  return PINNED_BUNDLE_KEYS.map((raw) => parseKey(raw)).filter((k): k is TrustedKey => k !== null)
}

export interface Session {
  readonly instance: Instance
  readonly held: Held
}

/** What the state needs from the outside; tests replace all of it. */
export interface TrustedDeps {
  fetchTrust(): Promise<{ doc: Buffer, sig: Buffer }>
  installNewer(store: Store, keys: () => readonly TrustedKey[]): Promise<unknown>
  discover(origin: URL): Promise<ServerInfo>
  now(): number
  vault: Vault | null
  root: string
  pinned: () => TrustedKey[]
  log(message: string): void
}

export function defaultDeps(vault: Vault | null): TrustedDeps {
  return {
    fetchTrust: () => fetchStatement(),
    installNewer: (store, keys) => installNewer(store, keys),
    discover: (origin) => discover(origin),
    now: () => Math.floor(Date.now() / 1000),
    vault,
    root: ROOT_PUB,
    pinned: pinnedKeys,
    log: (message) => console.error(`Mnema Desktop: ${message}`),
  }
}

export class Trusted {
  readonly store: Store
  readonly sockets = new Sockets()
  private keys: TrustedKey[]
  private readonly trust: TrustState
  /**
   * A fresh trust statement is in force and this round's fetch succeeded. False
   * at start: only a fetch in this process sets it, so a blocked network cannot
   * keep installing on the strength of a stored statement. Without it nothing
   * new is installed (freeze protection); the already verified bundles keep
   * running.
   */
  private freshRound = false
  private current: Session | null = null
  private readonly dataDir: string | null
  private readonly deps: TrustedDeps

  constructor(dataDir: string | null, builtin: Uint8Array | null, deps: TrustedDeps) {
    this.dataDir = dataDir
    this.deps = deps
    this.trust = TrustState.load(dataDir ? join(dataDir, 'web-trust') : null, rootKey(deps.root))
    this.keys = this.trust.effectiveKeys(deps.pinned())
    this.store = Store.load(dataDir ? join(dataDir, 'web-bundles') : null, builtin, this.keys)
  }

  keysInForce(): readonly TrustedKey[] {
    return this.keys
  }

  statement(): Statement | null {
    return this.trust.statement()
  }

  fresh(): boolean {
    return this.freshRound
  }

  /**
   * Takes a fetched trust statement. When it changes the keys in force, held
   * bundles are checked again and a running session on a withdrawn bundle is
   * ended. Returns whether bundles were dropped.
   */
  applyTrust(doc: Uint8Array, sig: Uint8Array, now: number): boolean {
    if (this.trust.accept(doc, sig, now) === 'unchanged') return false
    const statement = this.trust.statement()
    const caps = statement ? statementCaps(statement) : new Map()
    this.keys = this.trust.effectiveKeys(this.deps.pinned())
    const before = this.store.held().length
    this.store.reconcile(this.keys)
    this.store.applyCaps(caps)
    const dropped = this.store.held().length < before
    // A bundle withdrawn by a revoked key or a cap stops running, whatever withdrew it.
    const session = this.current
    if (session && !this.store.held().some((h) => h.bundle.keyId === session.held.bundle.keyId && compareVersions(h.bundle.version, session.held.bundle.version) === 0)) {
      this.sockets.closeAll()
      this.closeSession()
    }
    return dropped
  }

  /**
   * One trust round with the result of a fetch: applies the statement and
   * decides whether installs are allowed. Returns whether bundles were dropped.
   */
  applyRound(fetched: { doc: Buffer, sig: Buffer } | null, now: number): boolean {
    let dropped = false
    let ok = false
    // Offline or unreachable: the last good statement stays in force.
    if (fetched) {
      try {
        dropped = this.applyTrust(fetched.doc, fetched.sig, now)
        ok = true
      } catch (error) {
        if (!(error instanceof TrustError)) throw error
        this.deps.log(`trust statement refused: ${error.message}`)
      }
    }
    this.freshRound = ok && this.trust.fresh(now)
    return dropped
  }

  async trustRound(): Promise<boolean> {
    let fetched: { doc: Buffer, sig: Buffer } | null = null
    try {
      fetched = await this.deps.fetchTrust()
    } catch { /* offline */ }
    return this.applyRound(fetched, this.deps.now())
  }

  session(): Session | null {
    return this.current
  }

  closeSession(): void {
    this.current = null
  }

  /** The bundle new windows run: the selected instance's, else the newest. */
  view(): Held | undefined {
    return this.current ? this.current.held : this.store.newest()
  }

  /**
   * The verified bundle for this server. When the server is ahead of every
   * bundle held, look for an update first; never fall back to the server's own
   * JavaScript.
   */
  async choose(server: ServerInfo): Promise<Held> {
    const first = this.store.select(server)
    if (first.ok) return first.held
    if (first.refusal.kind === 'server_newer') {
      if (!this.fresh()) await this.trustRound()
      if (this.fresh() && !updatesDisabled()) {
        try { await this.deps.installNewer(this.store, () => this.keys) } catch { /* the refusal below says what is missing */ }
      }
      const again = this.store.select(server)
      if (again.ok) return again.held
      throw new Error(refusalText(again.refusal))
    }
    throw new Error(refusalText(first.refusal))
  }

  /** The account name in the secret store and the fallback file name of an instance's cookies. */
  jarName(origin: URL): string {
    return createHash('sha256').update(origin.href).digest('hex').slice(0, 16)
  }

  /** Checks the address, reads the server's health, picks the verified bundle and selects the instance. */
  async openInstance(address: string): Promise<Session> {
    const origin = parseAddress(address)
    const server = await this.deps.discover(origin)
    const held = await this.choose(server)
    if (this.current) throw new Error('Close the current instance before selecting another.')
    const name = this.jarName(origin)
    const jar = new Jar(this.dataDir ? join(this.dataDir, 'sessions', `${name}.json`) : null, this.deps.vault ? { vault: this.deps.vault, account: name } : null, this.deps.now())
    const session: Session = { instance: { origin, jar, limits: new Limits(), now: this.deps.now }, held }
    this.current = session
    return session
  }
}
