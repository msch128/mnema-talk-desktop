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
import { type Edit, Layout } from './layout.ts'
import { MAX_BADGE, badgeCount } from './notify.ts'
import { isLowerHex } from './encoding.ts'
import { MAX_MUTE_SECONDS, type Stored, liveMutes, shortcutPlace } from './instancestore.ts'
import { type Sleep, UNLOAD_CHOICES, boundedSleep, defaultSleep, storedSleep } from './sleep.ts'
import { type RailState, hue, initial } from './railstate.ts'
import type { Instance } from './proxy.ts'
import type { Sockets } from './sockets.ts'
import type { Held } from './store.ts'

/** What the badge and the rail show at most. */
export const MAX_COUNT = MAX_BADGE
export { MAX_IMPORT_BYTES, type Stored, emptyStored, loadStored, saveStored, shortcutPlace, storedFrom, storedText } from './instancestore.ts'
export type { RailState } from './railstate.ts'

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
  /** What the page last said: busy (a draft, an upload, a Talk) or idle; null until it says (an older bundle never does, and its page is never closed for being hidden). */
  busy: boolean | null
}

export class AddError extends Error {
  readonly kind: 'known'
  readonly id: string

  constructor(kind: 'known', id = '') {
    super('This server is already in the list.')
    this.kind = kind
    this.id = id
  }
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

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

function fresh(origin: URL): Slot {
  return { id: idFor(origin), origin, phase: { kind: 'starting' }, unread: 0, mentions: 0, reported: false, generation: randomBytes(16).toString('hex'), icon: null, mutedUntil: null, clicks: null, busy: null }
}

export class Registry {
  private slots: Slot[] = []
  /** The slots by id, so that a list of any length is looked up in constant time. */
  private readonly byId = new Map<string, Slot>()
  private layout = new Layout()
  private front: string | null = null
  private addingNow = false
  /** Whether the list this one was restored from was read in full (see `Stored.complete`). */
  private loadedInFull = true
  /** When hidden pages close. */
  sleep: Sleep = defaultSleep()

  static restore(stored: Stored): Registry {
    const registry = new Registry()
    registry.loadedInFull = stored.complete !== false
    registry.append(stored.servers)
    registry.layout = Layout.load(stored.items, stored.aliases, registry.ids(), registry.ids())
    registry.sort()
    for (const [id, until] of liveMutes(stored.muted, (x) => registry.contains(x), nowSeconds())) registry.mute(id, until)
    const wanted = stored.active ? idFor(stored.active) : null
    registry.front = wanted !== null && registry.contains(wanted) ? wanted : (registry.slots[0]?.id ?? null)
    registry.addingNow = registry.front === null
    registry.sleep = boundedSleep(stored.sleep)
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
    return this.byId.has(id)
  }

  ids(): string[] {
    return this.slots.map((s) => s.id)
  }

  slot(id: string): Slot | undefined {
    return this.byId.get(id)
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
    this.append([origin])
    this.layout.add(id)
    return id
  }

  /** Adds the servers of a list that are not here yet, at the end, in linear time; the layout is the caller's (`restore` and `merge` make it anew). */
  private append(origins: readonly URL[]): Slot[] {
    const added: Slot[] = []
    for (const origin of origins) {
      if (this.byId.has(idFor(origin))) continue
      const slot = fresh(origin)
      this.byId.set(slot.id, slot)
      this.slots.push(slot)
      added.push(slot)
    }
    return added
  }

  /** Takes a server out of the list and gives its slot back to be wiped. The neighbour takes over the front. */
  remove(id: string): Slot | null {
    const index = this.slots.findIndex((s) => s.id === id)
    if (index < 0) return null
    const [slot] = this.slots.splice(index, 1)
    this.byId.delete(id)
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

  /** The server a number key reaches (`shortcutPlace`): Ctrl+1 is the first, Ctrl+9 the last. */
  activateNth(number: number): string | null {
    const place = shortcutPlace(number, this.slots.length)
    const slot = place === null ? undefined : this.slots[place]
    if (!slot) return null
    this.activate(slot.id)
    return slot.id
  }

  startAdding(): void {
    this.addingNow = true
  }

  stopAdding(): void {
    this.addingNow = this.front === null
  }

  /** The servers stand in the order of the rail, so Ctrl+1 is the one at the top. */
  private sort(): void {
    const place = new Map(this.layout.flat().map((id, n) => [id, n]))
    this.slots.sort((a, b) => (place.get(a.id) ?? -1) - (place.get(b.id) ?? -1))
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
    const added = this.append(stored.servers)
    const here = this.layout
    const inFile = stored.servers.map(idFor)
    const named = new Set(inFile)
    const order = [...inFile, ...this.ids().filter((id) => !named.has(id))]
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
    slot.busy = null
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
      can_add: canAdd,
      items: structuredClone(this.layout.items),
      unload_after: this.sleep.afterSecs,
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
    return { servers: this.slots.map((s) => s.origin), active: front?.origin ?? null, items: structuredClone(this.layout.items), aliases: Object.fromEntries(this.layout.aliases), muted, sleep: storedSleep(this.sleep) }
  }

  /** The rail menu's "Unload idle servers": one of `UNLOAD_CHOICES`, the number of pages kept unchanged. Anything else is refused. */
  setUnloadAfter(afterSecs: number | null): boolean {
    if (!UNLOAD_CHOICES.includes(afterSecs)) return false
    this.sleep = { ...this.sleep, afterSecs }
    return true
  }

  /** The list as it is exported: the settings of this device stay here. */
  exported(): Stored {
    const list = this.stored()
    delete list.sleep
    return list
  }
}

/** The text of the menu entry for Ctrl+`number`: the 9 says it is the last. The same as the Tauri host. */
export function menuEntryText(number: number, name: string | null): string {
  if (number === 9) return name === null ? 'Last server' : `9 \u2014 ${name} (last)`
  return name === null ? `Server ${number}` : `${number} \u2014 ${name}`
}
