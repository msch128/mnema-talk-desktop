/**
 * What the host accepts for an OS notification. The text comes from a server
 * the user chose, so it is shown as plain text only, bounded, and never
 * trusted: control characters are flattened, ids must be plain identifiers and
 * toasts are rate limited. Pure, so it is tested without Electron.
 */
export const MAX_TITLE = 100
export const MAX_BODY = 240
export const MAX_ID = 64
export const MAX_BADGE = 999
/** Shortest gap between two toasts. */
export const MIN_GAP_MS = 300

export function cleanText(text: string, max: number): string {
  const flat = text.replace(/[\p{Cc}\p{Cf}]/gu, ' ').split(/\s+/u).filter((part) => part !== '').join(' ')
  const chars = Array.from(flat)
  if (chars.length <= max) return flat
  return `${chars.slice(0, Math.max(0, max - 1)).join('')}…`
}

export function validId(id: string): boolean {
  return id.length > 0 && id.length <= MAX_ID && /^[A-Za-z0-9-]+$/.test(id)
}

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
