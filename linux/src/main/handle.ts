/**
 * What the host's command channels have in common: the page a call comes from
 * is checked first, and the answer is an `ok` envelope or a short plain-text
 * failure.
 */
import { type IpcMainEvent, type IpcMainInvokeEvent, ipcMain } from 'electron'
import { failure } from '../core/ipcargs.ts'
import { parseUrl } from '../core/origin.ts'

/** The URL the main frame of the calling page shows; null for a call from any other frame, or from a page without one. */
export function mainFrameUrl(event: IpcMainInvokeEvent | IpcMainEvent): URL | null {
  const frame = event.senderFrame
  if (!frame || frame !== event.sender.mainFrame) return null
  return parseUrl(frame.url)
}

/**
 * Answers `{ command, args }` calls on a channel. `resolve` decides who is
 * calling (null: not allowed) and `run` does the command for that caller.
 */
export function handle<Caller>(
  channel: string,
  resolve: (event: IpcMainInvokeEvent) => Caller | null,
  run: (from: Caller, command: string, args: Record<string, unknown>) => Promise<unknown>,
): void {
  ipcMain.handle(channel, async (event, command: unknown, args: unknown) => {
    try {
      const from = resolve(event)
      if (from === null || typeof command !== 'string' || typeof args !== 'object' || args === null) throw new Error('Not allowed.')
      return { ok: true, value: await run(from, command, args as Record<string, unknown>) }
    } catch (error) {
      return failure(error)
    }
  })
}
