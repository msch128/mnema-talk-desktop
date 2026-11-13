/**
 * Runs a task once, a short while after the first of any number of requests:
 * many changes in a row (an import of thousands of servers, each of them
 * starting, the feeds reading their counts) become one run. A request while
 * the task is due adds nothing; one after it ran schedules the next run. The
 * same rule as the Tauri host's rail pushes (`web_shell/pushes.rs`).
 */
export class Coalesce {
  private due = false
  private readonly task: () => void
  private readonly delayMs: number
  private readonly later: (run: () => void, ms: number) => unknown

  constructor(task: () => void, delayMs: number, later: (run: () => void, ms: number) => unknown = setTimeout) {
    this.task = task
    this.delayMs = delayMs
    this.later = later
  }

  request(): void {
    if (this.due) return
    this.due = true
    this.later(() => {
      this.due = false
      this.task()
    }, this.delayMs)
  }
}

/** How long changes are gathered before the rail is pushed. */
export const PUSH_AFTER_MS = 150
