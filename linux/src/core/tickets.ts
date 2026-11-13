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

/** Lets at most `limit` tasks run at once; the others wait their turn, first come, first served. */
export class Turns {
  private running = 0
  private readonly waiting: (() => void)[] = []
  private readonly limit: number

  constructor(limit: number) {
    this.limit = limit
  }

  async run<T>(task: () => Promise<T>): Promise<T> {
    if (this.running >= this.limit) await new Promise<void>((resolve) => this.waiting.push(resolve))
    this.running++
    try {
      return await task()
    } finally {
      this.running--
      this.waiting.shift()?.()
    }
  }
}
