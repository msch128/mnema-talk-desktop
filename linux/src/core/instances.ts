/**
 * The servers this client is signed in to, side by side. A port of
 * `experimental/app/src/instances.rs`: every server is one slot with its own
 * session (cookie jar, upload limits, socket budget), its own browser storage
 * (a session partition, see `src/main`) and its own counters; nothing is shared
 * between slots. A page is tied to its slot by the session it runs in, never
 * by anything it says. This module is the bookkeeping, free of Electron so the
 * isolation rules can be tested.
 */
import { createHash, randomBytes } from 'node:crypto'
import { readFileSync, statSync } from 'node:fs'
import { writeFileAtomic } from './atomic.ts'
import { type Edit, type Item, Layout } from './layout.ts'
import { parseAddress } from './client.ts'
import { MAX_BADGE, badgeCount } from './notify.ts'
import { isLowerHex } from './encoding.ts'
import type { Instance } from './proxy.ts'
import type { Sockets } from './sockets.ts'
import type { Held } from './store.ts'

/** Ctrl+1 to Ctrl+9 reach the servers, so there are at most nine. */
export const MAX_INSTANCES = 9
/** What the badge and the rail show at most. */
export const MAX_COUNT = MAX_BADGE
const STORE_VERSION = 1
const MAX_STORE_BYTES = 32 * 1024
/** A mute may last a year and a day at most; -1 is for ever. */
const MAX_MUTE_SECONDS = 366 * 24 * 3600
/** What an imported list may weigh. */
export const MAX_IMPORT_BYTES = 64 * 1024

/** A server the client talks to and the verified bundle its pages run. */
export interface Session {
  readonly id: string
  readonly instance: Instance
  readonly held: Held
  /** What the server calls itself, when it says (`community_name` of `/api/health`). */
  readonly community?: string | null
  /** Whether the server's health answer leaves room for a community icon (see `core/icon.ts`). */
  readonly iconWanted?: boolean
  /** Sockets this server's pages hold open; the budget is per server. */
  readonly sockets: Sockets
}

/** Names the slot of a server: derived from its origin, so the same server is always the same slot. It also names its stored session. */
export function idFor(origin: URL): string {
  return createHash('sha256').update(origin.href).digest('hex').slice(0, 16)
}

export function validId(id: string): boolean {
  return isLowerHex(id, 16)
}

type Phase =
  | { kind: 'starting' }
  | { kind: 'ready', session: Session }
  | { kind: 'refused', why: string }

export interface Slot {
  readonly id: string
  readonly origin: URL
  phase: Phase
  /** Channels with unread messages that may notify, and unread mentions, as the page last reported them. */
  unread: number
  mentions: number
  /** The page reports both counts; until it does, the plain badge of an older bundle stands in for the mentions. */
  reported: boolean
  /** Names the run of this server's page: a toast keeps the generation it was shown for. */
  generation: string
  /** The version of the community icon on disk (see `core/icon.ts`), or none. */
  icon: string | null
  /** Notifications are off until then (epoch seconds), or for ever (-1); none: not muted. */
  mutedUntil: number | null
  /** The page asked for notifications: its callback channel and the next message index it expects. */
  clicks: { channel: number, index: number } | null
}

export class AddError extends Error {
  readonly kind: 'known' | 'full'
  readonly id: string

  constructor(kind: 'known' | 'full', id = '') {
    super(kind === 'known' ? 'This server is already in the list.' : `At most ${MAX_INSTANCES} servers fit in one window.`)
    this.kind = kind
    this.id = id
  }
}

/** What the rail draws for one server (the shape the rail page reads). */
interface RailInstance {
  id: string
  /** What the rail calls it: the local name if there is one, else `original`. */
  name: string
  /** What the server calls itself, or its address. */
  original: string
  /** The local name the person gave it. */
  alias: string | null
  initial: string
  hue: number
  status: 'starting' | 'ready' | 'refused'
  message: string | null
  unread: number
  mentions: number
  /** The version of the community icon (`rail_icon` fetches it), or none. */
  icon: string | null
  /** Muted until this time (epoch seconds), for ever (-1), or not muted. */
  muted_until: number | null
}

export interface RailState {
  active: string | null
  adding: boolean
  can_add: boolean
  /** In the order they stand on the rail. */
  instances: RailInstance[]
  /** The structure of the rail: servers on their own and folders. */
  items: Item[]
}

/** The address as a name: the host, with its port when it has one. */
export function slotName(origin: URL): string {
  return origin.port === '' ? origin.hostname : `${origin.hostname}:${origin.port}`
}

/**
 * The name on the rail, in menus and toasts: what a running server calls
 * itself (the Owner's community name), else its address. A server that only
 * repeats its host keeps the address with its port.
 */
export function nameOf(slot: Pick<Slot, 'origin' | 'phase'>): string {
  const community = slot.phase.kind === 'ready' ? slot.phase.session.community : null
  return community && community.toLowerCase() !== slot.origin.hostname.toLowerCase() ? community : slotName(slot.origin)
}

function initial(name: string): string {
  const first = /[\p{L}\p{N}]/u.exec(name.replace(/^www\./, ''))
  return first ? first[0].toUpperCase() : '?'
}

function hue(id: string): number {
  return (Number.parseInt(id.slice(0, 4), 16) || 0) % 360
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
function liveMutes(raw: unknown, known: (id: string) => boolean, now: number): Map<string, number> {
  const out = new Map<string, number>()
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return out
  for (const [id, until] of Object.entries(raw)) {
    if (known(id) && typeof until === 'number' && Number.isFinite(until) && (until === -1 || (until > now && until <= now + MAX_MUTE_SECONDS))) out.set(id, Math.trunc(until))
  }
  return out
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

function fresh(origin: URL): Slot {
  return { id: idFor(origin), origin, phase: { kind: 'starting' }, unread: 0, mentions: 0, reported: false, generation: randomBytes(16).toString('hex'), icon: null, mutedUntil: null, clicks: null }
}

export class Registry {
  private slots: Slot[] = []
  private layout = new Layout()
  private front: string | null = null
  private addingNow = false
  /** Whether the list this one was restored from was read in full (see `Stored.complete`). */
  private loadedInFull = true

  static restore(stored: Stored): Registry {
    const registry = new Registry()
    registry.loadedInFull = stored.complete !== false
    for (const origin of stored.servers) {
      try { registry.add(origin) } catch { /* a duplicate */ }
    }
    registry.layout = Layout.load(stored.items, stored.aliases, registry.ids(), registry.ids())
    registry.sort()
    for (const [id, until] of liveMutes(stored.muted, (x) => registry.contains(x), nowSeconds())) registry.mute(id, until)
    const wanted = stored.active ? idFor(stored.active) : null
    registry.front = wanted !== null && registry.contains(wanted) ? wanted : (registry.slots[0]?.id ?? null)
    registry.addingNow = registry.front === null
    return registry
  }

  get length(): number {
    return this.slots.length
  }

  /** False when the list at start may have been shorter than what was saved: what is missing from it proves nothing (it is not a reason to delete a server's data). */
  get readInFull(): boolean {
    return this.loadedInFull
  }

  contains(id: string): boolean {
    return this.slots.some((s) => s.id === id)
  }

  ids(): string[] {
    return this.slots.map((s) => s.id)
  }

  slot(id: string): Slot | undefined {
    return this.slots.find((s) => s.id === id)
  }

  /** The running session of a server, or none if it is not (or no longer) in the list, still starting or refused. */
  session(id: string): Session | null {
    const phase = this.slot(id)?.phase
    return phase?.kind === 'ready' ? phase.session : null
  }

  active(): string | null {
    return this.front
  }

  adding(): boolean {
    return this.addingNow
  }

  add(origin: URL): string {
    const id = idFor(origin)
    if (this.contains(id)) throw new AddError('known', id)
    if (this.slots.length >= MAX_INSTANCES) throw new AddError('full')
    this.slots.push(fresh(origin))
    this.layout.add(id)
    return id
  }

  /** Takes a server out of the list and gives its slot back to be wiped. The neighbour takes over the front. */
  remove(id: string): Slot | null {
    const index = this.slots.findIndex((s) => s.id === id)
    if (index < 0) return null
    const [slot] = this.slots.splice(index, 1)
    this.layout.remove(id)
    this.sort()
    if (this.front === id) this.front = this.slots[Math.min(index, this.slots.length - 1)]?.id ?? null
    this.addingNow = this.slots.length === 0 || (this.addingNow && this.front !== null)
    return slot ?? null
  }

  activate(id: string): boolean {
    if (!this.contains(id)) return false
    this.front = id
    this.addingNow = false
    return true
  }

  /** Ctrl+1 is the first server. */
  activateNth(number: number): string | null {
    const slot = Number.isInteger(number) && number >= 1 ? this.slots[number - 1] : undefined
    if (!slot) return null
    this.activate(slot.id)
    return slot.id
  }

  startAdding(): boolean {
    if (this.slots.length >= MAX_INSTANCES) return false
    this.addingNow = true
    return true
  }

  stopAdding(): void {
    this.addingNow = this.front === null
  }

  /** The servers stand in the order of the rail, so Ctrl+1 is the one at the top. */
  private sort(): void {
    const order = this.layout.flat()
    this.slots.sort((a, b) => order.indexOf(a.id) - order.indexOf(b.id))
  }

  /** Changes how the servers sit on the rail. `folder` is the id a new folder gets. False: refused, nothing changed. */
  edit(edit: Edit, folder: string): boolean {
    if (!this.layout.apply(edit, folder)) return false
    this.sort()
    return true
  }

  hasFolder(id: string): boolean {
    return this.layout.hasFolder(id)
  }

  /** What the rail calls a server: the local name if it has one, else what the server calls itself. */
  displayName(slot: Pick<Slot, 'id' | 'origin' | 'phase'>): string {
    return this.layout.aliases.get(slot.id) ?? nameOf(slot)
  }

  /** Turns a server's notifications off until `until` (epoch seconds; -1 for ever), or on again (0). False: refused, nothing changed. */
  mute(id: string, until: number, now = nowSeconds()): boolean {
    const slot = this.slot(id)
    if (!slot || !Number.isFinite(until)) return false
    if (until === 0) slot.mutedUntil = null
    else if (until === -1 || (until > now && until <= now + MAX_MUTE_SECONDS)) slot.mutedUntil = Math.trunc(until)
    else return false
    return true
  }

  /** Whether a server's notifications are off at `now`. */
  isMuted(id: string, now = nowSeconds()): boolean {
    return this.mutedAt(this.slot(id), now) !== null
  }

  private mutedAt(slot: Slot | undefined, now: number): number | null {
    const until = slot?.mutedUntil ?? null
    return until === -1 || (until !== null && until > now) ? until : null
  }

  /**
   * Takes in a list somebody exported: servers that are not here join at the
   * end (as far as there is room) and are returned, aliases and mutes are
   * applied, and the layout of the file is laid over all of them. The server in
   * front and the pages stay as they are.
   */
  merge(stored: Stored, now = nowSeconds()): Slot[] {
    const added: Slot[] = []
    for (const origin of stored.servers) {
      try {
        const id = this.add(origin)
        const slot = this.slot(id)
        if (slot) added.push(slot)
      } catch { /* known, or no room */ }
    }
    const here = this.layout
    const inFile = stored.servers.map(idFor)
    const order = [...inFile, ...this.ids().filter((id) => !inFile.includes(id))].filter((id) => this.contains(id))
    const aliases = { ...Object.fromEntries(here.aliases), ...(typeof stored.aliases === 'object' && stored.aliases !== null && !Array.isArray(stored.aliases) ? stored.aliases : {}) }
    this.layout = Layout.load(stored.items ?? structuredClone(here.items), aliases, this.ids(), order)
    this.sort()
    for (const [id, until] of liveMutes(stored.muted, (x) => this.contains(x), now)) this.mute(id, until, now)
    return added
  }

  setIcon(id: string, token: string | null): boolean {
    const slot = this.slot(id)
    if (!slot || slot.icon === token) return false
    slot.icon = token
    return true
  }

  /** The counts a page reported. Both numbers are bounded, whatever the page sent. */
  report(id: string, unread: number, mentions: number): boolean {
    const slot = this.slot(id)
    if (!slot) return false
    slot.unread = badgeCount(unread)
    slot.mentions = badgeCount(mentions)
    slot.reported = true
    return true
  }

  /** The plain badge of a bundle that does not report both numbers: its mentions, and a dot when there are any. */
  reportBadge(id: string, count: number): boolean {
    const slot = this.slot(id)
    if (!slot) return false
    if (slot.reported) return true
    slot.mentions = badgeCount(count)
    slot.unread = slot.mentions > 0 ? 1 : 0
    return true
  }

  /** A new run of a server's page: counters and notification state start over. */
  restart(id: string): void {
    const slot = this.slot(id)
    if (!slot) return
    slot.unread = 0
    slot.mentions = 0
    slot.reported = false
    slot.clicks = null
    slot.generation = randomBytes(16).toString('hex')
  }

  /** Mentions over all servers that are not muted: the number on the launcher icon. */
  badge(now = nowSeconds()): number {
    return Math.min(MAX_COUNT, this.slots.reduce((sum, s) => sum + (this.mutedAt(s, now) === null ? s.mentions : 0), 0))
  }

  /** The slot a toast belongs to, by the generation it was shown for. */
  byGeneration(generation: string): Slot | undefined {
    return generation === '' ? undefined : this.slots.find((s) => s.generation === generation)
  }

  rail(canAdd: boolean, now = nowSeconds()): RailState {
    return {
      active: this.front,
      adding: this.addingNow,
      can_add: canAdd && this.slots.length < MAX_INSTANCES,
      items: structuredClone(this.layout.items),
      instances: this.slots.map((slot) => {
        const original = nameOf(slot)
        const alias = this.layout.aliases.get(slot.id) ?? null
        const name = alias ?? original
        return {
          id: slot.id,
          name,
          original,
          alias,
          initial: initial(name),
          hue: hue(slot.id),
          status: slot.phase.kind,
          message: slot.phase.kind === 'refused' ? slot.phase.why : null,
          unread: slot.unread,
          mentions: slot.mentions,
          icon: slot.icon,
          muted_until: this.mutedAt(slot, now),
        }
      }),
    }
  }

  stored(): Stored {
    const front = this.front ? this.slot(this.front) : undefined
    const now = nowSeconds()
    const muted = Object.fromEntries(this.slots.flatMap((s) => { const until = this.mutedAt(s, now); return until === null ? [] : [[s.id, until]] }))
    return { servers: this.slots.map((s) => s.origin), active: front?.origin ?? null, items: structuredClone(this.layout.items), aliases: Object.fromEntries(this.layout.aliases), muted }
  }
}
