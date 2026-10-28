/**
 * Numbers the runs of something that may be started again before the first run
 * ends (a server start), so that only the newest run goes on with its result.
 */
export class Tickets {
  private readonly latest = new Map<string, number>()
  private next = 0

  /** Starts a run of `key`; every earlier run of it is no longer current. */
  begin(key: string): number {
    const ticket = ++this.next
    this.latest.set(key, ticket)
    return ticket
  }

  current(key: string, ticket: number): boolean {
    return this.latest.get(key) === ticket
  }
}
