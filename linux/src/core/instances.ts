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
import { mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { parseAddress } from './client.ts'
import { MAX_BADGE, badgeCount } from './notify.ts'
import type { Instance } from './proxy.ts'
import type { Sockets } from './sockets.ts'
import type { Held } from './store.ts'

/** Ctrl+1 to Ctrl+9 reach the servers, so there are at most nine. */
export const MAX_INSTANCES = 9
/** What the badge and the rail show at most. */
export const MAX_COUNT = MAX_BADGE
const STORE_VERSION = 1
const MAX_STORE_BYTES = 16 * 1024

/** A server the client talks to and the verified bundle its pages run. */
export interface Session {
  readonly id: string
  readonly instance: Instance
  readonly held: Held
  /** Sockets this server's pages hold open; the budget is per server. */
  readonly sockets: Sockets
}

/** Names the slot of a server: derived from its origin, so the same server is always the same slot. It also names its stored session. */
export function idFor(origin: URL): string {
  return createHash('sha256').update(origin.href).digest('hex').slice(0, 16)
}

export function validId(id: string): boolean {
  return /^[0-9a-f]{16}$/.test(id)
}

export type Phase =
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
export interface RailInstance {
  id: string
  name: string
  initial: string
  hue: number
  status: 'starting' | 'ready' | 'refused'
  message: string | null
  unread: number
  mentions: number
}

export interface RailState {
  active: string | null
  adding: boolean
  can_add: boolean
  instances: RailInstance[]
}

export function slotName(origin: URL): string {
  return origin.port === '' ? origin.hostname : `${origin.hostname}:${origin.port}`
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
}

export function emptyStored(): Stored {
  return { servers: [], active: null }
}

/** Reads the list. Anything that does not hold up (unknown version, a bad address, a duplicate, too many) is dropped rather than trusted. */
export function loadStored(file: string | null): Stored {
  if (!file) return emptyStored()
  let raw: unknown
  try {
    if (statSync(file).size > MAX_STORE_BYTES) return emptyStored()
    raw = JSON.parse(readFileSync(file, 'utf8'))
  } catch {
    return emptyStored()
  }
  if (typeof raw !== 'object' || raw === null) return emptyStored()
  const { version, servers, active } = raw as Record<string, unknown>
  if (version !== STORE_VERSION || !Array.isArray(servers)) return emptyStored()
  const list: URL[] = []
  for (const address of servers) {
    if (typeof address !== 'string' || list.length >= MAX_INSTANCES) continue
    try {
      const origin = parseAddress(address)
      if (!list.some((u) => u.href === origin.href)) list.push(origin)
    } catch { /* dropped */ }
  }
  let front: URL | null = null
  if (typeof active === 'string') {
    try { front = list.find((u) => u.href === parseAddress(active).href) ?? null } catch { front = null }
  }
  return { servers: list, active: front }
}

export function saveStored(file: string | null, stored: Stored): void {
  if (!file) return
  try {
    mkdirSync(dirname(file), { recursive: true })
    const temporary = `${file}.tmp`
    writeFileSync(temporary, JSON.stringify({ version: STORE_VERSION, servers: stored.servers.map((u) => u.href), active: stored.active?.href ?? null }), { mode: 0o600 })
    renameSync(temporary, file)
  } catch { /* the list is a convenience; the sessions do not depend on it */ }
}

function fresh(origin: URL): Slot {
  return { id: idFor(origin), origin, phase: { kind: 'starting' }, unread: 0, mentions: 0, reported: false, generation: randomBytes(16).toString('hex'), clicks: null }
}

export class Registry {
  private slots: Slot[] = []
  private front: string | null = null
  private addingNow = false
  static restore(stored: Stored): Registry {
    const registry = new Registry()
    for (const origin of stored.servers) {
      try { registry.add(origin) } catch { /* a duplicate */ }
    }
    const wanted = stored.active ? idFor(stored.active) : null
    registry.front = wanted !== null && registry.contains(wanted) ? wanted : (registry.slots[0]?.id ?? null)
    registry.addingNow = registry.front === null
    return registry
  }

  get length(): number {
    return this.slots.length
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
    return id
  }

  /** Takes a server out of the list and gives its slot back to be wiped. The neighbour takes over the front. */
  remove(id: string): Slot | null {
    const index = this.slots.findIndex((s) => s.id === id)
    if (index < 0) return null
    const [slot] = this.slots.splice(index, 1)
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

  moveTo(id: string, to: number): boolean {
    const from = this.slots.findIndex((s) => s.id === id)
    if (from < 0) return false
    const [slot] = this.slots.splice(from, 1)
    this.slots.splice(Math.min(Math.max(0, Math.trunc(to)), this.slots.length), 0, slot as Slot)
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

  /** Mentions over all servers: the number on the launcher icon. */
  badge(): number {
    return Math.min(MAX_COUNT, this.slots.reduce((sum, s) => sum + s.mentions, 0))
  }

  /** The slot a toast belongs to, by the generation it was shown for. */
  byGeneration(generation: string): Slot | undefined {
    return generation === '' ? undefined : this.slots.find((s) => s.generation === generation)
  }

  rail(canAdd: boolean): RailState {
    return {
      active: this.front,
      adding: this.addingNow,
      can_add: canAdd && this.slots.length < MAX_INSTANCES,
      instances: this.slots.map((slot) => {
        const name = slotName(slot.origin)
        return {
          id: slot.id,
          name,
          initial: initial(name),
          hue: hue(slot.id),
          status: slot.phase.kind,
          message: slot.phase.kind === 'refused' ? slot.phase.why : null,
          unread: slot.unread,
          mentions: slot.mentions,
        }
      }),
    }
  }

  stored(): Stored {
    const front = this.front ? this.slot(this.front) : undefined
    return { servers: this.slots.map((s) => s.origin), active: front?.origin ?? null }
  }
}
