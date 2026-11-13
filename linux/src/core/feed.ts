/**
 * The rail's counts of a server whose page is not loaded (never opened in this
 * run, or closed after hiding for a while; see `sleep.ts`). The same as the
 * Tauri host's `web_feed`: `GET /api/read-state` through the server's own
 * session, read again when its socket says something changed (at most every
 * few seconds) and once a minute in any case. The socket is the host's own and
 * only listens: right after it opens it says the member is idle, so it shows
 * the member as away, as the hidden page it replaces did, and it sends
 * nothing else.
 */
import { forward, type Instance } from './proxy.ts'
import { counts } from './sleep.ts'
import { Sockets, type WsEvent } from './sockets.ts'

export const SETTLE_MS = 2000
export const EVERY_MS = 60_000
export const REOPEN_MS = 30_000

/** What the host's socket says right after it opens: the member is idle here. */
export const IDLE = '{"type":"presence_idle","payload":{"idle":true}}'

const CHANGES = new Set(['message_create', 'message_update', 'message_delete', 'read_state', 'channels_changed', 'permissions_changed', 'roles_changed', 'member_roles_changed'])

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

/** Reads a server's counts once; null when it did not answer with them. */
export async function readCounts(instance: Instance): Promise<{ unread: number, mentions: number } | null> {
  const resp = await forward(instance, 'GET', '/api/read-state', [], new Uint8Array())
  if (resp.status !== 200) return null
  try {
    return counts(JSON.parse(Buffer.from(resp.body).toString('utf8')))
  } catch {
    return null
  }
}

export interface FeedTimes { settleMs: number, everyMs: number, reopenMs: number }

/** One server's feed: started when its page goes, stopped when a page takes over. */
export class Feed {
  private readonly sockets = new Sockets()
  private timer: NodeJS.Timeout | null = null
  private due = 0
  private stopped = false
  private socket: number | null = null

  private readonly instance: Instance
  private readonly report: (counts: { unread: number, mentions: number }) => void
  private readonly times: FeedTimes

  constructor(instance: Instance, report: (counts: { unread: number, mentions: number }) => void, times: FeedTimes = { settleMs: SETTLE_MS, everyMs: EVERY_MS, reopenMs: REOPEN_MS }) {
    this.instance = instance
    this.report = report
    this.times = times
  }

  start(): void {
    this.listen()
    void this.refresh()
  }

  stop(): void {
    this.stopped = true
    if (this.timer) clearTimeout(this.timer)
    this.sockets.closeAll()
  }

  private listen(): void {
    if (this.stopped) return
    try {
      this.socket = this.sockets.connect(this.instance, false, (event) => this.on(event))
    } catch {
      this.again(this.times.reopenMs)
    }
  }

  private on(event: WsEvent): boolean {
    if (this.stopped) return false
    if (event.kind === 'open') this.sockets.send(event.id, IDLE)
    else if (event.kind === 'message') { if (changesCounts(event.data)) this.again(this.times.settleMs) }
    else { this.socket = null; this.again(this.times.reopenMs) }
    return true
  }

  /** Reads the counts after `ms` (an earlier read already due stays), opening a closed socket again first. */
  private again(ms: number): void {
    if (this.stopped) return
    const at = Date.now() + ms
    if (this.timer && this.due <= at) return
    if (this.timer) clearTimeout(this.timer)
    this.due = at
    this.timer = setTimeout(() => {
      this.timer = null
      if (this.socket === null) this.listen()
      void this.refresh()
    }, ms)
  }

  private async refresh(): Promise<void> {
    const got = await readCounts(this.instance).catch(() => null)
    if (this.stopped) return
    if (got) this.report(got)
    if (!this.timer) this.again(this.times.everyMs)
  }
}
