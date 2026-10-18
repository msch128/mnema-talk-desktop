/**
 * A minimal Chrome DevTools Protocol client for the end-to-end run: it lists
 * the pages of the launched app and evaluates expressions in them. Only a
 * development run (an unpackaged Electron) can be driven this way; the packaged
 * app has the remote debugging switches turned off by its fuses.
 */
import WebSocket from 'ws'

export interface Target { id: string, title: string, url: string, type: string, webSocketDebuggerUrl: string }

export async function pages(port: number): Promise<Target[]> {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`)
  return ((await response.json()) as Target[]).filter((t) => t.type === 'page')
}

export class Page {
  private readonly ws: WebSocket
  private next = 0
  private readonly waiting = new Map<number, { resolve(value: unknown): void, reject(error: Error): void }>()
  readonly opened: Promise<void>
  readonly target: Target
  /** Console errors, uncaught exceptions and browser log entries once `watch` was called. */
  readonly problems: string[] = []

  constructor(target: Target) {
    this.target = target
    this.ws = new WebSocket(target.webSocketDebuggerUrl)
    this.opened = new Promise((resolve, reject) => { this.ws.once('open', () => resolve()); this.ws.once('error', reject) })
    this.ws.on('message', (data) => {
      const message = JSON.parse(data.toString()) as { id?: number, result?: unknown, error?: { message: string }, method?: string, params?: Record<string, unknown> }
      if (message.method === 'Log.entryAdded') {
        const entry = message.params?.entry as { level: string, text: string, url?: string } | undefined
        if (entry && (entry.level === 'error' || entry.level === 'warning')) this.problems.push(`${entry.level}: ${entry.text} ${entry.url ?? ''}`)
      } else if (message.method === 'Runtime.exceptionThrown') {
        const details = message.params?.exceptionDetails as { text: string, exception?: { description?: string } } | undefined
        this.problems.push(`exception: ${details?.exception?.description ?? details?.text ?? ''}`)
      }
      if (message.id === undefined) return
      const entry = this.waiting.get(message.id)
      this.waiting.delete(message.id)
      if (message.error) entry?.reject(new Error(message.error.message))
      else entry?.resolve(message.result)
    })
  }

  async send(method: string, params: object = {}): Promise<unknown> {
    await this.opened
    const id = ++this.next
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject })
      this.ws.send(JSON.stringify({ id, method, params }))
    })
  }

  /** Starts collecting `problems`. */
  async watch(): Promise<void> {
    await this.send('Log.enable')
    await this.send('Runtime.enable')
  }

  /** Evaluates an expression in the page (a promise is awaited) and returns its JSON value. */
  async evaluate<T>(expression: string): Promise<T> {
    const result = (await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })) as {
      result: { value?: T }, exceptionDetails?: { text: string, exception?: { description?: string } }
    }
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text)
    return result.result.value as T
  }

  close(): void {
    this.ws.close()
  }
}
