/**
 * The commands of the server rail. They have their own IPC channel, and only
 * the rail's own page (its main frame, showing the app origin) is answered:
 * the pages of the servers and the selector cannot reach the list, whatever
 * they send.
 */
import type { IpcMainInvokeEvent } from 'electron'
import { type Args, wholeNumber } from '../core/ipcargs.ts'
import { localOrigin } from '../core/origin.ts'
import { validId } from '../core/instances.ts'
import type { Context } from './context.ts'
import { handle, mainFrameUrl } from './handle.ts'
import { activate, cancelAdding, moveServer, railSnapshot, reconnect, removeServer, startAdding } from './shell.ts'

function fromRail(ctx: Context, event: IpcMainInvokeEvent): boolean {
  const rail = ctx.shell.rail
  if (!rail || rail.webContents.isDestroyed() || event.sender.id !== rail.webContents.id) return false
  const url = mainFrameUrl(event)
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
      moveServer(ctx, serverId(args), wholeNumber(args, 'to'))
      return null
    }
    case 'rail_remove': await removeServer(ctx, serverId(args)); return null
    case 'rail_reconnect': await reconnect(ctx, serverId(args)); return null
    default: throw new Error('Unknown command.')
  }
}

export function registerRailIpc(ctx: Context): void {
  handle('mnema:rail', (event) => fromRail(ctx, event) || null, (_from, command, args) => run(ctx, command, args))
}
