/**
 * A cap on the sockets the feeds hold open. With any number of servers, a
 * socket for every server without a page would use up the open files of the
 * process and put a socket on every server for nothing. At most `MAX_LIVE`
 * feeds keep one: those of the servers in use most recently. A feed beyond
 * that reads its counts on its timer (every minute) instead, and takes a place
 * again when one is free at its next read. The same rule as the Tauri host
 * (`web_feed/budget.rs`).
 */

/** The most feed sockets open at once. */
export const MAX_LIVE = 64

/** What the budget can close: the sockets of one feed. */
export interface Closable { closeAll(): void }

/** The places taken: by server, how recently it was in use (higher: later) and the sockets of its feed. */
export class Budget {
  private readonly held = new Map<string, { rank: number, sockets: Closable }>()
  private readonly cap: number

  constructor(cap: number) {
    this.cap = cap
  }

  /** Whether the feed of `id` (its `sockets`) may open its socket. When all places are taken, the server in use longest ago gives its place up if this one was in use later: its socket is closed and its feed reads on its timer from then on. */
  take(id: string, rank: number, sockets: Closable): boolean {
    if (this.held.get(id)?.sockets === sockets) return true
    if (this.held.size >= this.cap) {
      let oldest: [string, number] | null = null
      for (const [key, place] of this.held) if (oldest === null || place.rank < oldest[1]) oldest = [key, place.rank]
      if (oldest === null || oldest[1] >= rank) return false
      this.held.get(oldest[0])?.sockets.closeAll()
      this.held.delete(oldest[0])
    }
    this.held.set(id, { rank, sockets })
    return true
  }

  /** The socket of the feed of `id` closed, or the feed ended. */
  giveBack(id: string, sockets: Closable): void {
    if (this.held.get(id)?.sockets === sockets) this.held.delete(id)
  }

  holders(): string[] {
    return [...this.held.keys()].sort()
  }
}

/** What one feed asks the budget with. */
export interface Gate { readonly id: string, readonly rank: number, readonly budget: Budget }

const shared = new Budget(MAX_LIVE)
let latest = 0

/** The gate of a feed starting now: the server was in use more recently than those whose feeds started before. */
export function gateFor(id: string): Gate {
  latest += 1
  return { id, rank: latest, budget: shared }
}

/** A gate that always has a place (a feed on its own, in the tests). */
export const OPEN_GATE: Gate = { id: '', rank: 0, budget: new Budget(Number.POSITIVE_INFINITY) }
