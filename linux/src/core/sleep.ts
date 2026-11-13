/**
 * Servers without a loaded page: what the rail shows for them and which hidden
 * pages may be closed. The same rules as the Tauri host (`src/rail_sleep.rs`),
 * tested against `counts` and `unload` in `test-vectors/rail-layout.json`.
 */
import { MAX_INSTANCES } from './instancestore.ts'

/** Hidden for this long (seconds), a page may be closed. A setting. */
export const DEFAULT_UNLOAD_AFTER_SECS = 10 * 60
/** Pages that keep running whatever their age, the one in front included. A setting. */
export const DEFAULT_KEEP_LOADED = 4
const AFTER_MIN = 60
const AFTER_MAX = 86_400

const whole = (v: unknown): number => (typeof v === 'number' && Number.isSafeInteger(v) && v > 0 ? v : 0)

/**
 * The rail's two numbers for a server from its `GET /api/read-state`
 * (`states`) and `GET /api/dms` (`dms`, its conversations), as the web app's
 * `aggregateActivity` makes them over both: mentions over every channel and
 * conversation (a mention counts in a muted one too), and those with unread
 * messages that are not muted (a conversation's `inherit` is not muted).
 * Anything but a positive whole number counts as zero.
 */
export function counts(states: unknown, dms: unknown = []): { unread: number, mentions: number } {
  let unread = 0
  let mentions = 0
  const all = [states, dms].flatMap((list) => (Array.isArray(list) ? (list as unknown[]) : []))
  for (const state of all) {
    if (typeof state !== 'object' || state === null || Array.isArray(state)) continue
    const s = state as Record<string, unknown>
    mentions += whole(s.mention_count)
    if (s.notify_level !== 'mute' && whole(s.unread_count) > 0) unread++
  }
  return { unread, mentions }
}

/** How hidden pages are treated: closed after `afterSecs` hidden (null: never) while more than `keep` run. */
export interface Sleep { afterSecs: number | null, keep: number }

export const defaultSleep = (): Sleep => ({ afterSecs: DEFAULT_UNLOAD_AFTER_SECS, keep: DEFAULT_KEEP_LOADED })

/** The settings as they are written to the list file. */
export const storedSleep = (sleep: Sleep): { after_secs: number | null, keep: number } => ({ after_secs: sleep.afterSecs, keep: sleep.keep })

/** The settings as stored (`after_secs`, `keep`: the Tauri host's names), held to their bounds; nonsense is the default. */
export function boundedSleep(raw: unknown): Sleep {
  const d = defaultSleep()
  if (typeof raw !== 'object' || raw === null) return d
  const { after_secs: afterSecs, keep } = raw as Record<string, unknown>
  const after = afterSecs === null ? null : Number.isSafeInteger(afterSecs) ? Math.min(Math.max(afterSecs as number, AFTER_MIN), AFTER_MAX) : d.afterSecs
  const kept = Number.isSafeInteger(keep) ? Math.min(Math.max(keep as number, 1), MAX_INSTANCES) : d.keep
  return { afterSecs: after, keep: kept }
}

/** Whether a page may be closed at all, whatever its age: not the server in front, and only once it said it is idle. Asked again right before a page is closed. */
export const closable = (inFront: boolean, busy: boolean | null): boolean => !inFront && busy === false

/** The choices of "Unload idle servers" in the rail's menu, in seconds hidden; null is Off. The number of pages kept stays as it is. */
export const UNLOAD_CHOICES: readonly (number | null)[] = [null, 600, 3600]

/** One server as the rule sees it. `busy`: true a draft, an upload or a Talk; false idle; null the page never said. */
export interface PageState { id: string, active: boolean, page: boolean, hiddenSince: number | null, busy: boolean | null }

/**
 * The pages to close now, oldest hidden first, while more than `keep` run.
 * Never the server in front, never one hidden for less than `afterSecs`,
 * never one whose page has not said it is idle.
 */
export function pagesToUnload(sleep: Sleep, now: number, servers: readonly PageState[]): string[] {
  if (sleep.afterSecs === null) return []
  const after = sleep.afterSecs
  let running = servers.filter((s) => s.page).length
  const candidates = servers
    .filter((s) => s.page && closable(s.active, s.busy) && s.hiddenSince !== null && now - s.hiddenSince >= after)
    .sort((a, b) => (a.hiddenSince as number) - (b.hiddenSince as number) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  const out: string[] = []
  for (const s of candidates) {
    if (running <= sleep.keep) break
    out.push(s.id)
    running--
  }
  return out
}
