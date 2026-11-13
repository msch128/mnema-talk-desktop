/**
 * The rail's counts of a server whose page is not loaded (never opened in this
 * run, or closed after hiding for a while; see `sleep.ts`). The same as the
 * Tauri host's `web_feed`: `GET /api/read-state` and `GET /api/dms` through
 * the server's own session, read again when its socket says something changed (at most every
 * few seconds) and once a minute in any case. The socket is the host's own and
 * only listens: right after it opens it says the member is idle, so it shows
 * the member as away, as the hidden page it replaces did, and it sends
 * nothing else.
 */
import { type Gate, OPEN_GATE } from './feedbudget.ts'
import type { Registry } from './instances.ts'
import { forward, type Instance } from './proxy.ts'
import { counts } from './sleep.ts'
import { Sockets, type WsEvent } from './sockets.ts'

export const SETTLE_MS = 2000
export const EVERY_MS = 60_000
export const REOPEN_MS = 30_000
/** An unreachable server is asked again after twice as long each time, up to this. */
export const REOPEN_MAX_MS = 300_000

/** What the host's socket says right after it opens: the member is idle here. */
export const IDLE = '{"type":"presence_idle","payload":{"idle":true}}'

const CHANGES = new Set(['message_create', 'message_update', 'message_delete', 'read_state', 'channels_changed', 'permissions_changed', 'roles_changed', 'member_roles_changed', 'dm_created', 'dm_member_added', 'dm_member_removed', 'dm_prefs_updated'])

/** The events after which the counts may have changed. */
export function changesCounts(frame: string): boolean {
  try {
    const value: unknown = JSON.parse(frame)
    const kind = typeof value === 'object' && value !== null ? (value as { type?: unknown }).type : undefined
    return typeof kind === 'string' && CHANGES.has(kind)
  } catch {
    return false
  }
}

/** What one read of a server gave: the rail's two numbers, a session that is gone (401, 403), or no answer that could be read. */
export type Read = { kind: 'counts', unread: number, mentions: number } | { kind: 'signed_out' } | { kind: 'unreachable' }

const SIGNED_OUT: Read = { kind: 'signed_out' }
const UNREACHABLE: Read = { kind: 'unreachable' }

/** One answer as JSON; a 404 (a server without direct messages) is null. */
async function getJson(instance: Instance, path: string): Promise<{ value: unknown } | Read> {
  const resp = await forward(instance, 'GET', path, [], new Uint8Array())
  if (resp.status === 401 || resp.status === 403) return SIGNED_OUT
  if (resp.status === 404) return { value: null }
  if (resp.status !== 200) return UNREACHABLE
  try {
    return { value: JSON.parse(Buffer.from(resp.body).toString('utf8')) as unknown }
  } catch {
    return UNREACHABLE
  }
}

/** Reads a server's read state and conversations once, as the page counts them. The same as the Tauri host. */
export async function readCounts(instance: Instance): Promise<Read> {
  const states = await getJson(instance, '/api/read-state')
  if ('kind' in states) return states
  if (states.value === null) return UNREACHABLE
  const dms = await getJson(instance, '/api/dms')
  if ('kind' in dms) return dms
  return { kind: 'counts', ...counts(states.value, dms.value ?? []) }
}

export interface FeedTimes { settleMs: number, everyMs: number, reopenMs: number, reopenMaxMs: number }

const TIMES: FeedTimes = { settleMs: SETTLE_MS, everyMs: EVERY_MS, reopenMs: REOPEN_MS, reopenMaxMs: REOPEN_MAX_MS }

/** The wait before the `failures`th retry of an unreachable server: doubling from `reopenMs` up to `reopenMaxMs`, up to a fifth earlier (`random`, 0 to 1) so that many servers do not ask at the same moment. The same as the Tauri host. */
export function backoffMs(times: FeedTimes, failures: number, random: number): number {
  const base = Math.min(times.reopenMs * 2 ** Math.min(Math.max(failures - 1, 0), 16), times.reopenMaxMs)
  return base - Math.floor((base / 5) * Math.min(Math.max(random, 0), 0.999))
}

/** One server's feed: started when its page goes, stopped when a page takes over or the session is gone. */
export class Feed {
  private readonly sockets = new Sockets()
  private timer: NodeJS.Timeout | null = null
  private due = 0
  private stopped = false
  private socket: number | null = null
  private failures = 0

  private readonly instance: Instance
  private readonly report: (read: Read) => void
  private readonly times: FeedTimes
  private readonly gate: Gate

  constructor(instance: Instance, report: (read: Read) => void, times: FeedTimes = TIMES, gate: Gate = OPEN_GATE) {
    this.instance = instance
    this.report = report
    this.times = times
    this.gate = gate
  }

  start(): void {
    void this.refresh()
  }

  stop(): void {
    this.stopped = true
    if (this.timer) clearTimeout(this.timer)
    this.sockets.closeAll()
    this.gate.budget.giveBack(this.gate.id, this.sockets)
  }

  get running(): boolean {
    return !this.stopped
  }

  private listen(): void {
    if (this.stopped) return
    // No place for a socket (`feedbudget.ts`): the counts are read on the timer.
    if (!this.gate.budget.take(this.gate.id, this.gate.rank, this.sockets)) return
    try {
      // With the conversation query: direct messages count on the rail too.
      this.socket = this.sockets.connect(this.instance, true, (event) => this.on(event))
    } catch {
      this.gate.budget.giveBack(this.gate.id, this.sockets)
      this.again(this.times.reopenMs)
    }
  }

  private on(event: WsEvent): boolean {
    if (this.stopped) return false
    if (event.kind === 'open') this.sockets.send(event.id, IDLE)
    else if (event.kind === 'message') { if (changesCounts(event.data)) this.again(this.times.settleMs) }
    else {
      // A close of an earlier socket says nothing about the one open now.
      if (event.id !== this.socket) return true
      this.socket = null
      this.gate.budget.giveBack(this.gate.id, this.sockets)
      this.again(this.failures > 0 ? backoffMs(this.times, this.failures, Math.random()) : this.times.reopenMs)
    }
    return true
  }

  /** Reads the counts after `ms` (an earlier read already due stays). */
  private again(ms: number): void {
    if (this.stopped) return
    const at = Date.now() + ms
    if (this.timer && this.due <= at) return
    if (this.timer) clearTimeout(this.timer)
    this.due = at
    this.timer = setTimeout(() => {
      this.timer = null
      void this.refresh()
    }, ms)
  }

  private async refresh(): Promise<void> {
    const read = await readCounts(this.instance).catch((): Read => UNREACHABLE)
    if (this.stopped) return
    this.report(read)
    if (read.kind === 'signed_out') { this.stop(); return }
    if (read.kind === 'unreachable') {
      this.failures++
      this.sockets.closeAll()
      this.socket = null
      this.gate.budget.giveBack(this.gate.id, this.sockets)
      if (this.timer) clearTimeout(this.timer)
      this.timer = null
      this.again(backoffMs(this.times, this.failures, Math.random()))
      return
    }
    this.failures = 0
    // The socket opens only once the session has answered: a server that is signed out or down is never sent a socket.
    if (this.socket === null) this.listen()
    if (!this.timer) this.again(this.times.everyMs)
  }
}

/** How many new mentions a quiet toast announces: one toast for each rise of the count, none for a muted server. The same as the Tauri host. */
export function mentionRise(before: number, after: number, muted: boolean): number | null {
  return !muted && after > before ? after - before : null
}

/** What the quiet toast says. */
export function mentionText(count: number, server: string): string {
  return `${count} new ${count === 1 ? 'mention' : 'mentions'} on ${server}`
}

/** What one read did to the server's slot: the server is gone (the feed ends), nothing on the rail changed (no answer, or the server is not running), or the counts are on the rail, with a rise of the mentions to announce (null: none). */
export type Applied = { kind: 'gone' } | { kind: 'kept' } | { kind: 'shown', rise: number | null, name: string }

/** Puts what was read on the slot of `id`; a signed-out session clears its counts. The same as the Tauri host's `apply`. */
export function applyRead(registry: Registry, id: string, read: Read): Applied {
  if (read.kind === 'unreachable') return { kind: 'kept' }
  const slot = registry.slot(id)
  if (!slot) return { kind: 'gone' }
  if (!registry.session(id)) return { kind: 'kept' }
  // A session that is gone shows nothing until its page signs in again.
  const { unread, mentions } = read.kind === 'counts' ? read : { unread: 0, mentions: 0 }
  // The first numbers since the server started (or since a restart) are what is there already, not news: they announce nothing.
  const [before, first] = [slot.mentions, !slot.reported]
  registry.report(id, unread, mentions)
  const name = registry.displayName(slot)
  return { kind: 'shown', rise: mentionRise(before, slot.mentions, registry.isMuted(id) || first), name }
}

/** Forgets the feed of `id` if it is `feed`: a feed that ends never removes a newer one of the same server (its page failed to open meanwhile). */
export function forget(feeds: Map<string, Feed>, id: string, feed: Feed): void {
  if (feeds.get(id) === feed) feeds.delete(id)
}
