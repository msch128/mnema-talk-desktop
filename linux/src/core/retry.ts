import { INVALID_RESPONSE, UNAVAILABLE } from './discover.ts'

/**
 * A server that could not be reached when the client started is tried again on
 * its own, so a computer that woke up before its network does not leave the
 * tab on "refused" until someone presses Reconnect. A port of
 * `experimental/app/src/web_shell/retry.rs`.
 */
const FIRST_MS = 5_000
const LONGEST_MS = 5 * 60_000

/** How long to wait after `attempt` automatic retries have failed: 5 s, doubling, at most 5 min. */
export function backoffMs(attempt: number): number {
  return Math.min(FIRST_MS * 2 ** Math.min(attempt, 16), LONGEST_MS)
}

/**
 * Whether a refusal says the server could not be reached or asked (offline,
 * DNS, timeout, a refused connection, an answer that is not the health check),
 * which another try later can fix. A certificate or TLS problem, a server that
 * needs an update and a withdrawn bundle stay refused until someone acts.
 */
export function isTransportFailure(why: string): boolean {
  return [
    'The instance refused the connection.',
    'The instance did not answer in time.',
    'The instance address could not be resolved.',
    'Unable to reach the instance using trusted HTTPS.',
    UNAVAILABLE,
    'Instance check failed.',
    INVALID_RESPONSE,
  ].some((known) => why.startsWith(known))
}

/** The retries waiting, one per server: a new one replaces the old, and removing or reconnecting a server cancels it. */
export class Retries {
  private readonly timers = new Map<string, NodeJS.Timeout>()

  /** Runs `start` with the number of the next retry after the wait for `attempt`. */
  schedule(id: string, attempt: number, start: (next: number) => void): void {
    this.cancel(id)
    const timer = setTimeout(() => { this.timers.delete(id); start(attempt + 1) }, backoffMs(attempt))
    timer.unref()
    this.timers.set(id, timer)
  }

  cancel(id: string): void {
    const timer = this.timers.get(id)
    if (timer) clearTimeout(timer)
    this.timers.delete(id)
  }
}

export const retries = new Retries()
