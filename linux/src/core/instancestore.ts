/**
 * The list of servers at rest: addresses only, no session and no name a page
 * chose. Read back with every address checked exactly like one typed by hand;
 * anything that does not hold up is dropped rather than trusted.
 */
import { readFileSync, statSync } from 'node:fs'
import { writeFileAtomic } from './atomic.ts'
import { parseAddress } from './client.ts'

/** Ctrl+1 to Ctrl+9 reach the servers, so there are at most nine. */
export const MAX_INSTANCES = 9
const STORE_VERSION = 1
const MAX_STORE_BYTES = 32 * 1024
/** A mute may last a year and a day at most; -1 is for ever. */
export const MAX_MUTE_SECONDS = 366 * 24 * 3600
/** What an imported list may weigh. */
export const MAX_IMPORT_BYTES = 64 * 1024

/** The list at rest: addresses only. No session, no name the page chose. */
export interface Stored {
  servers: URL[]
  active: URL | null
  /** The layout as written (`Layout.load` makes sense of it): absent in a list from before folders. */
  items?: unknown
  aliases?: unknown
  /** Server id to the time (epoch seconds, -1 for ever) its notifications are off until. */
  muted?: unknown
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
  const { version, servers, active, items, aliases, muted } = raw as Record<string, unknown>
  if (version !== STORE_VERSION || !Array.isArray(servers)) return null
  const list: URL[] = []
  let complete = true
  for (const address of servers) {
    if (typeof address !== 'string' || list.length >= MAX_INSTANCES) { complete = false; continue }
    try {
      const origin = parseAddress(address)
      if (!list.some((u) => u.href === origin.href)) list.push(origin)
    } catch { complete = false }
  }
  let front: URL | null = null
  if (typeof active === 'string') {
    try { front = list.find((u) => u.href === parseAddress(active).href) ?? null } catch { front = null }
  }
  return complete ? { servers: list, active: front, items, aliases, muted } : { ...partial(list, front), items, aliases, muted }
}

/** The list as it is written to disk, and as it is exported: addresses, names, layout and mutes, nothing else. */
export function storedText(stored: Stored): string {
  return JSON.stringify({
    version: STORE_VERSION,
    servers: stored.servers.map((u) => u.href),
    active: stored.active?.href ?? null,
    ...(stored.items !== undefined ? { items: stored.items } : {}),
    ...(stored.aliases !== undefined ? { aliases: stored.aliases } : {}),
    ...(stored.muted !== undefined ? { muted: stored.muted } : {}),
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
