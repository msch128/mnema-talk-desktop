/**
 * The vocabulary of the self-updater: its phases, what the update window shows
 * and what it needs from its surroundings.
 */
import { MENU_DE, MENU_EN } from './menutext.ts'
import type { FetchOptions } from './updates.ts'
import type { Statement } from './trust.ts'

/** `waiting`: downloaded and verified, installs once the Talk has ended. */
export type Phase = 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'waiting' | 'installing' | 'failed'
/** `after_call`: download and verify now, install when the Talk has ended. */
export type Mode = 'now' | 'later' | 'after_call'
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
  /** "Update now" was asked for during a Talk: the page asks whether to wait for its end. */
  confirm_call: boolean
  /** The person skipped this version: the rail shows no pill for it. */
  skipped: boolean
}

/** What the update entries of the app menu say, as in the Tauri host (`native_updates/menu.rs`). A dot marks a waiting update. */
export interface MenuView { title: string, now: string, nowEnabled: boolean, check: string, version: string }

export function menuView(snapshot: Snapshot, german: boolean): MenuView {
  const offered = ['available', 'ready', 'waiting', 'failed'].includes(snapshot.phase) && snapshot.version !== null
  const text = german ? MENU_DE : MENU_EN
  return { title: offered ? text.offered : text.idle, now: text.now, nowEnabled: offered, check: text.check, version: `Mnema Desktop ${snapshot.current}` }
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
  /** Whether the person is in a Talk right now; without it, never. */
  inTalk?(): boolean
  /** How often a waiting update looks whether the Talk has ended (tests shorten it). */
  talkPollMs?: number
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
