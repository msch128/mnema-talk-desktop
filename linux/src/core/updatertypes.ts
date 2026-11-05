/**
 * The vocabulary of the self-updater: its phases, what the update window shows
 * and what it needs from its surroundings.
 */
import type { FetchOptions } from './updates.ts'
import type { Statement } from './trust.ts'

export type Phase = 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'installing' | 'failed'
export type Mode = 'now' | 'later'
/** `untrusted`: no current trust statement could be fetched, so nothing new is installed (the signature is not at fault). */
export type Failure = 'offline' | 'verify' | 'untrusted' | 'install' | 'unknown'

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

export class FailureError extends Error {
  readonly failure: Failure

  constructor(failure: Failure) {
    super(failure)
    this.failure = failure
  }
}

export function classify(error: unknown): Failure {
  if (error instanceof FailureError) return error.failure
  return 'offline'
}

export function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timeout')), ms)
    promise.then((value) => { clearTimeout(timer); resolve(value) }, (error: unknown) => { clearTimeout(timer); reject(error instanceof Error ? error : new Error(String(error))) })
  })
}
