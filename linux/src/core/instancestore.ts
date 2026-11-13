/**
 * The list of servers at rest: addresses only, no session and no name a page
 * chose. Read back with every address checked exactly like one typed by hand;
 * anything that does not hold up is dropped rather than trusted.
 */
import { readFileSync, statSync } from 'node:fs'
import { writeFileAtomic } from './atomic.ts'
import { parseAddress } from './client.ts'

const STORE_VERSION = 1
/**
 * What the list file may weigh. There is no limit on the number of servers:
 * this only keeps a damaged file from being read whole into memory; 8 MiB is
 * room for 100 000 servers and more. The same bound as the Tauri host.
 */
const MAX_STORE_BYTES = 8 << 20
/** A mute may last a year and a day at most; -1 is for ever. */
export const MAX_MUTE_SECONDS = 366 * 24 * 3600
/** What an imported list may weigh: the same bound as the list file, and the one the rail's import dialog asks for. */
export const MAX_IMPORT_BYTES = 8 << 20

/** Where Ctrl+`number` goes among `count` servers: 1 to 8 the server at that place, 9 the last one (as browsers do with tabs). */
export function shortcutPlace(number: number, count: number): number | null {
  if (!Number.isInteger(number) || !Number.isInteger(count)) return null
  if (number >= 1 && number <= 8 && number <= count) return number - 1
  return number === 9 && count > 0 ? count - 1 : null
}

/** The list at rest: addresses only. No session, no name the page chose. */
export interface Stored {
  servers: URL[]
  active: URL | null
  /** The layout as written (`Layout.load` makes sense of it): absent in a list from before folders. */
  items?: unknown
  aliases?: unknown
  /** Server id to the time (epoch seconds, -1 for ever) its notifications are off until. */
  muted?: unknown
  /** When hidden pages close (`boundedSleep` makes sense of it); never part of an export. */
  sleep?: unknown
  /** False when the file was there but could not be read in full (unreadable, too large, unknown version, an address that did not hold up): the list may be shorter than what the person had. */
  complete?: boolean
}

export function emptyStored(): Stored {
  return { servers: [], active: null }
}

/** A list that stands for something lost: nothing may be concluded from what is missing in it. */
function partial(servers: URL[] = [], active: URL | null = null): Stored {
  return { servers, active, complete: false }
}

/** Reads the list. Anything that does not hold up (unknown version, a bad address, a duplicate, too many) is dropped rather than trusted. */
export function loadStored(file: string | null): Stored {
  if (!file) return emptyStored()
  let raw: unknown
  try {
    if (statSync(file).size > MAX_STORE_BYTES) return partial()
    raw = JSON.parse(readFileSync(file, 'utf8'))
  } catch (error) {
    // No file is a list that was never written; any other failure is a list that exists and could not be read.
    return (error as NodeJS.ErrnoException).code === 'ENOENT' ? emptyStored() : partial()
  }
  return storedFrom(raw) ?? partial()
}

/** The list in a JSON value; null when it is not a list of this version at all. Addresses are checked exactly like those typed in by hand. */
export function storedFrom(raw: unknown): Stored | null {
  if (typeof raw !== 'object' || raw === null) return null
  const { version, servers, active, items, aliases, muted, sleep } = raw as Record<string, unknown>
  if (version !== STORE_VERSION || !Array.isArray(servers)) return null
  const list: URL[] = []
  let complete = true
  const seen = new Set<string>()
  for (const address of servers) {
    if (typeof address !== 'string') { complete = false; continue }
    try {
      const origin = parseAddress(address)
      if (!seen.has(origin.href)) { seen.add(origin.href); list.push(origin) }
    } catch { complete = false }
  }
  let front: URL | null = null
  if (typeof active === 'string') {
    try { front = list.find((u) => u.href === parseAddress(active).href) ?? null } catch { front = null }
  }
  return complete ? { servers: list, active: front, items, aliases, muted, sleep } : { ...partial(list, front), items, aliases, muted, sleep }
}

/** The list as it is written to disk (with the settings of this device) and as it is exported (without them): addresses, names, layout and mutes, nothing else. */
export function storedText(stored: Stored): string {
  return JSON.stringify({
    version: STORE_VERSION,
    servers: stored.servers.map((u) => u.href),
    active: stored.active?.href ?? null,
    ...(stored.items !== undefined ? { items: stored.items } : {}),
    ...(stored.aliases !== undefined ? { aliases: stored.aliases } : {}),
    ...(stored.muted !== undefined ? { muted: stored.muted } : {}),
    ...(stored.sleep !== undefined ? { sleep: stored.sleep } : {}),
  })
}

export function saveStored(file: string | null, stored: Stored): void {
  if (!file) return
  try {
    writeFileAtomic(file, Buffer.from(storedText(stored)), { dirMode: 0o700 })
  } catch { /* the list is a convenience; the sessions do not depend on it */ }
}

/** The mutes of a stored list that still stand at `now`, for the servers in `known`. */
export function liveMutes(raw: unknown, known: (id: string) => boolean, now: number): Map<string, number> {
  const out = new Map<string, number>()
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return out
  for (const [id, until] of Object.entries(raw)) {
    if (known(id) && typeof until === 'number' && Number.isFinite(until) && (until === -1 || (until > now && until <= now + MAX_MUTE_SECONDS))) out.set(id, Math.trunc(until))
  }
  return out
}
