/**
 * The commands of the server rail. They have their own IPC channel, and only
 * the rail's own page (its main frame, showing the app origin) is answered:
 * the pages of the servers and the selector cannot reach the list, whatever
 * they send.
 */
import { type IpcMainInvokeEvent, ipcMain } from 'electron'
import { localOrigin, parseUrl } from '../core/origin.ts'
import { validId } from '../core/instances.ts'
import type { Context } from './context.ts'
import { activate, cancelAdding, moveServer, railSnapshot, reconnect, removeServer, startAdding } from './shell.ts'

type Args = Record<string, unknown>

function fromRail(ctx: Context, event: IpcMainInvokeEvent): boolean {
  const rail = ctx.shell.rail
  const frame = event.senderFrame
  if (!rail || rail.webContents.isDestroyed() || event.sender.id !== rail.webContents.id || !frame || frame !== event.sender.mainFrame) return false
  const url = parseUrl(frame.url)
  return url !== null && localOrigin(url) !== null
}

function serverId(args: Args): string {
  const value = args.id
  if (typeof value !== 'string' || !validId(value)) throw new Error('invalid arguments')
  return value
}

async function run(ctx: Context, command: string, args: Args): Promise<unknown> {
  switch (command) {
    case 'rail_state': return railSnapshot(ctx)
    case 'rail_activate': activate(ctx, serverId(args)); return null
    case 'rail_add': startAdding(ctx); return null
    case 'rail_cancel_add': cancelAdding(ctx); return null
    case 'rail_move': {
      const to = args.to
      if (typeof to !== 'number' || !Number.isSafeInteger(to) || to < 0) throw new Error('invalid arguments')
      moveServer(ctx, serverId(args), to)
      return null
    }
    case 'rail_remove': await removeServer(ctx, serverId(args)); return null
    case 'rail_reconnect': await reconnect(ctx, serverId(args)); return null
    default: throw new Error('Unknown command.')
  }
}

export function registerRailIpc(ctx: Context): void {
  ipcMain.handle('mnema:rail', async (event, command: unknown, args: unknown) => {
    try {
      if (!fromRail(ctx, event) || typeof command !== 'string' || typeof args !== 'object' || args === null) throw new Error('Not allowed.')
      return { ok: true, value: await run(ctx, command, args as Args) }
    } catch (error) {
      return { ok: false, error: (error instanceof Error ? error.message : 'The command failed.').slice(0, 300) }
    }
  })
}
