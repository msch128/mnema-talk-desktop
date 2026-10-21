/**
 * The commands the pages may call, over one IPC channel. The page-facing names
 * and shapes are those of the Tauri host (`desktop_open_instance`,
 * `desktop_ws_*`, `native_update_*`), so the signed bundle runs unchanged.
 * Every call is checked against the window it comes from: its kind, that it is
 * the main frame, and the URL that frame shows.
 */
import { type IpcMainEvent, type IpcMainInvokeEvent, type WebContents, ipcMain, shell } from 'electron'
import { bundled, localOrigin, parseUrl, pickerUi, updateUi } from '../core/origin.ts'
import { RELEASES_PAGE } from '../core/pinned.ts'
import type { WsEvent } from '../core/sockets.ts'
import type { Context, Role } from './context.ts'
import { pickerChoose, pickerSources } from './permissions.ts'
import { notify, notifyOpen, notifyPermission, setBadge } from './notify.ts'
import { openInstance } from './windows.ts'

type Args = Record<string, unknown>

/** The window this call comes from, if it is a main frame showing a page it may show. */
function caller(ctx: Context, event: IpcMainInvokeEvent | IpcMainEvent): { role: Role, contents: WebContents } | null {
  const contents = event.sender
  const frame = event.senderFrame
  const role = ctx.roles.get(contents.id)
  if (!role || !frame || frame !== contents.mainFrame) return null
  const url = parseUrl(frame.url)
  if (!url) return null
  const ok = role === 'selector' ? bundled(url)
    : role === 'instance' ? localOrigin(url) !== null
      : role === 'updater' ? updateUi(url)
        : role === 'picker' ? pickerUi(url)
          : false
  return ok ? { role, contents } : null
}

/** The identity the verified bundle announces to the page, before any of its scripts run. */
function identity(ctx: Context, event: IpcMainEvent): object | null {
  const from = caller(ctx, event)
  if (!from || (from.role !== 'selector' && from.role !== 'instance')) return null
  const held = ctx.trusted.view()
  if (!held) return null
  return {
    version: held.bundle.version.join('.'),
    source: held.source,
    webClientApi: held.bundle.webClientApi,
    keyId: held.bundle.keyId,
    commit: held.bundle.commit,
    role: from.role === 'selector' ? 'selector' : 'instance',
    // What the host offers this window beyond the common surface; the web app detects it instead of a version bump.
    capabilities: from.role === 'instance' ? ['notify'] : [],
  }
}

function text(args: Args, name: string): string {
  const value = args[name]
  if (typeof value !== 'string') throw new Error('invalid arguments')
  return value
}

function id(args: Args, name: string): number {
  const value = args[name]
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error('invalid arguments')
  return value
}

async function run(ctx: Context, role: Role, contents: WebContents, command: string, args: Args): Promise<unknown> {
  const { trusted } = ctx
  const only = (...roles: Role[]) => { if (!roles.includes(role)) throw new Error('Not allowed.') }
  switch (command) {
    case 'desktop_open_instance': {
      if (role !== 'selector') throw new Error('Only the instance selector may open an instance.')
      if (ctx.windows.instance) throw new Error('Close the current instance before selecting another.')
      await trusted.openInstance(text(args, 'address'))
      try {
        const selector = ctx.windows.selector
        openInstance(ctx, () => {
          trusted.sockets.closeAll()
          trusted.closeSession()
          const attachment = ctx.windows.attachment
          if (attachment && !attachment.isDestroyed()) attachment.close()
          const back = ctx.windows.selector
          if (back && !back.isDestroyed()) back.show()
        })
        selector?.hide()
      } catch {
        trusted.closeSession()
        throw new Error('Unable to open the instance.')
      }
      return null
    }
    case 'desktop_ws_open': {
      only('instance')
      const session = trusted.session()
      if (!session) throw new Error('No instance is selected.')
      if (text(args, 'path') !== '/api/ws') throw new Error('Only /api/ws can be opened.')
      const channel = id(args, 'onEvent')
      let index = 0
      const send = (message: object) => {
        if (contents.isDestroyed()) return false
        contents.send('mnema:callback', channel, message)
        return true
      }
      return trusted.sockets.connect(session.instance, (event: WsEvent) => {
        const delivered = send({ index: index++, message: event })
        if (event.kind === 'close') send({ index: index++, end: true })
        return delivered
      })
    }
    case 'desktop_ws_send':
      only('instance')
      trusted.sockets.send(id(args, 'id'), text(args, 'data'))
      return null
    case 'desktop_ws_close':
      only('instance')
      trusted.sockets.close(id(args, 'id'))
      return null
    case 'desktop_notify_open':
      only('instance')
      return notifyOpen(contents, id(args, 'onEvent'))
    case 'desktop_notify':
      only('instance')
      return notify(ctx, contents, text(args, 'title'), text(args, 'body'), text(args, 'channelId'), text(args, 'messageId'))
    case 'desktop_notify_permission':
    case 'desktop_notify_request_permission':
      only('instance')
      // Electron asks the desktop's notification service, never the person: there is nothing to request.
      return notifyPermission()
    case 'desktop_set_badge':
      only('instance')
      return setBadge(id(args, 'count'))
    case 'native_update_state':
      only('updater')
      return ctx.updater?.snapshot() ?? null
    case 'native_update_action': {
      only('updater')
      const updater = ctx.updater
      if (!updater) throw new Error('Updates are not available.')
      const external = updater.snapshot().external
      switch (text(args, 'action')) {
        case 'check': void updater.check(true); break
        case 'now': if (external) void shell.openExternal(RELEASES_PAGE); else void updater.install('now'); break
        case 'later': if (external) void shell.openExternal(RELEASES_PAGE); else void updater.install('later'); break
        case 'download_page': void shell.openExternal(RELEASES_PAGE); break
        case 'skip': updater.skip(); break
        case 'close': ctx.windows.updater?.close(); break
        default: throw new Error('Unknown action.')
      }
      return updater.snapshot()
    }
    case 'picker_sources':
      only('picker')
      return pickerSources(ctx)
    case 'picker_choose':
      only('picker')
      pickerChoose(ctx, args.id === null ? null : text(args, 'id'))
      return null
    default:
      throw new Error('Unknown command.')
  }
}

export function registerIpc(ctx: Context): void {
  ipcMain.handle('mnema:invoke', async (event, command: unknown, args: unknown) => {
    const from = caller(ctx, event)
    try {
      if (!from || typeof command !== 'string' || typeof args !== 'object' || args === null) throw new Error('Not allowed.')
      return { ok: true, value: await run(ctx, from.role, from.contents, command, args as Args) }
    } catch (error) {
      // Plain text only: no stack, no path.
      return { ok: false, error: (error instanceof Error ? error.message : 'The command failed.').slice(0, 300) }
    }
  })
  ipcMain.on('mnema:identity', (event) => {
    event.returnValue = identity(ctx, event)
  })
}
