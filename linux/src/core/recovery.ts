/**
 * Whether a page whose renderer ended may be reloaded. A page that dies right
 * after every reload is left alone, so a crash loop cannot spin the CPU.
 */
export class RendererRecovery {
  private readonly times: number[] = []
  private readonly max: number
  private readonly windowMs: number

  constructor(max = 3, windowMs = 60_000) {
    this.max = max
    this.windowMs = windowMs
  }

  /** True if the page should be reloaded now; records the attempt. */
  allow(reason: string, now: number): boolean {
    // A page that exited on its own is not a crash.
    if (reason === 'clean-exit') return false
    while (this.times.length > 0 && now - this.times[0] >= this.windowMs) this.times.shift()
    if (this.times.length >= this.max) return false
    this.times.push(now)
    return true
  }
}
