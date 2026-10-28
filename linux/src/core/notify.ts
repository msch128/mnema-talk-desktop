/**
 * What the host accepts for an OS notification. The text comes from a server
 * the user chose, so it is shown as plain text only, bounded, and never
 * trusted: control characters are flattened, ids must be plain identifiers and
 * toasts are rate limited. Pure, so it is tested without Electron.
 */
export const MAX_TITLE = 100
export const MAX_BODY = 240
export const MAX_ID = 64
/** What the launcher badge and the rail show at most. */
export const MAX_BADGE = 999
/** Shortest gap between two toasts. */
export const MIN_GAP_MS = 300

export function cleanText(text: string, max: number): string {
  const flat = text.replace(/[\p{Cc}\u200E\u200F\u202A-\u202E\u2066-\u2069\uFEFF]/gu, ' ').split(/\s+/u).filter((part) => part !== '').join(' ')
  const chars = Array.from(flat)
  if (chars.length <= max) return flat
  return `${chars.slice(0, Math.max(0, max - 1)).join('')}…`
}

export function validId(id: string): boolean {
  return id.length > 0 && id.length <= MAX_ID && /^[A-Za-z0-9-]+$/.test(id)
}

/** A count as the badge and the rail show it: a whole number from 0 to MAX_BADGE. */
export function badgeCount(count: number): number {
  return Number.isFinite(count) ? Math.min(MAX_BADGE, Math.max(0, Math.trunc(count))) : 0
}

/** Allows a toast at most every MIN_GAP_MS. */
export class ToastGate {
  private last = Number.NEGATIVE_INFINITY

  allow(now: number): boolean {
    if (now - this.last < MIN_GAP_MS) return false
    this.last = now
    return true
  }
}

export const MAX_SERVER_NAME = 60

/** The title with the name of the server behind it, once there is more than one: a toast says which server it is from. */
export function taggedTitle(title: string, name: string, several: boolean): string {
  const clean = cleanText(title, MAX_TITLE)
  return several ? `${clean} \u00b7 ${cleanText(name, MAX_SERVER_NAME)}` : clean
}

/** One gate per server: a server that floods does not hold up the others. */
export class ToastGates {
  private readonly gates = new Map<string, ToastGate>()

  allow(server: string, now: number): boolean {
    let gate = this.gates.get(server)
    if (!gate) { gate = new ToastGate(); this.gates.set(server, gate) }
    return gate.allow(now)
  }

  forget(server: string): void {
    this.gates.delete(server)
  }
}

/** What a menu entry of the Servers menu does. */
export type ServerMenuAction = { kind: 'nth', number: number } | { kind: 'add' }

export function serverMenuAction(id: string): ServerMenuAction | null {
  if (id === 'server-add') return { kind: 'add' }
  const match = /^server-([1-9])$/.exec(id)
  return match ? { kind: 'nth', number: Number(match[1]) } : null
}
