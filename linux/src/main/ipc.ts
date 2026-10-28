/**
 * The commands the pages may call, over one IPC channel. The page-facing names
 * and shapes are those of the Tauri host (`desktop_open_instance`,
 * `desktop_ws_*`, `native_update_*`), so the signed bundle runs unchanged.
 * Every call is checked against the window it comes from: its kind, that it is
 * the main frame, and the URL that frame shows.
 */
import { type IpcMainEvent, type IpcMainInvokeEvent, type WebContents, ipcMain, shell } from 'electron'
import { type Args, wholeNumber } from '../core/ipcargs.ts'
import { socketRequest, type WsEvent } from '../core/sockets.ts'
import { bundled, localOrigin, pickerUi, updateUi } from '../core/origin.ts'
import { RELEASES_PAGE } from '../core/pinned.ts'
import type { Context, Role } from './context.ts'
import { handle, mainFrameUrl } from './handle.ts'
import { pickerChoose, pickerSources } from './permissions.ts'
import { notify, notifyOpen, notifyPermission } from './notify.ts'
import { addServer, countsChanged } from './shell.ts'

/** The page this call comes from, with the server it belongs to (decided by the host when it made the page). */
interface Caller { role: Role, contents: WebContents, server: string | null }

function caller(ctx: Context, event: IpcMainInvokeEvent | IpcMainEvent): Caller | null {
  const contents = event.sender
  const role = ctx.roles.get(contents.id)
  const url = mainFrameUrl(event)
  if (!role || !url) return null
  const ok = role === 'selector' ? bundled(url)
    : role === 'instance' ? localOrigin(url) !== null
      : role === 'updater' ? updateUi(url)
        : role === 'picker' ? pickerUi(url)
          : false
  const server = ctx.servers.get(contents.id) ?? null
  return ok && (role !== 'instance' || server !== null) ? { role, contents, server } : null
}

/** The identity the verified bundle announces to the page, before any of its scripts run. */
function identity(ctx: Context, event: IpcMainEvent): object | null {
  const from = caller(ctx, event)
  if (!from || (from.role !== 'selector' && from.role !== 'instance')) return null
  // A server's page runs the bundle its server was matched with; the selector runs the one in front.
  const held = from.server !== null ? ctx.trusted.session(from.server)?.held : ctx.trusted.view()
  if (!held) return null
  return {
    version: held.bundle.version.join('.'),
    source: held.source,
    webClientApi: held.bundle.webClientApi,
    keyId: held.bundle.keyId,
    commit: held.bundle.commit,
    role: from.role === 'selector' ? 'selector' : 'instance',
    // What the host offers this window beyond the common surface; the web app detects it instead of a version bump.
    // `unread`: the page reports unread channels and mentions for the server rail. `ws_query`: the socket may ask for conversation events.
    capabilities: from.role === 'instance' ? ['notify', 'unread', 'ws_query'] : [],
  }
}

function text(args: Args, name: string): string {
  const value = args[name]
  if (typeof value !== 'string') throw new Error('invalid arguments')
  return value
}

async function run(ctx: Context, from: Caller, command: string, args: Args): Promise<unknown> {
  const { role, contents } = from
  const only = (...roles: Role[]) => { if (!roles.includes(role)) throw new Error('Not allowed.') }
  /** The running session of the server this page belongs to, and nothing else. */
  const own = () => {
    const session = from.server === null ? null : ctx.trusted.session(from.server)
    if (!session) throw new Error('No instance is selected.')
    return session
  }
  switch (command) {
    case 'desktop_open_instance':
      if (role !== 'selector') throw new Error('Only the instance selector may open an instance.')
      await addServer(ctx, text(args, 'address'))
      return null
    case 'desktop_ws_open': {
      only('instance')
      const session = own()
      const dm = socketRequest(text(args, 'path'))
      const channel = wholeNumber(args, 'onEvent')
      let index = 0
      const send = (message: object) => {
        if (contents.isDestroyed()) return false
        contents.send('mnema:callback', channel, message)
        return true
      }
      return session.sockets.connect(session.instance, dm, (event: WsEvent) => {
        const delivered = send({ index: index++, message: event })
        if (event.kind === 'close') send({ index: index++, end: true })
        return delivered
      })
    }
    case 'desktop_ws_send':
      only('instance')
      own().sockets.send(wholeNumber(args, 'id'), text(args, 'data'))
      return null
    case 'desktop_ws_close':
      only('instance')
      own().sockets.close(wholeNumber(args, 'id'))
      return null
    case 'desktop_notify_open':
      only('instance')
      own()
      return notifyOpen(ctx, from.server as string, contents, wholeNumber(args, 'onEvent'))
    case 'desktop_notify':
      only('instance')
      own()
      return notify(ctx, from.server as string, text(args, 'title'), text(args, 'body'), text(args, 'channelId'), text(args, 'messageId'))
    case 'desktop_notify_permission':
    case 'desktop_notify_request_permission':
      only('instance')
      // Electron asks the desktop's notification service, never the person: there is nothing to request.
      return notifyPermission()
    case 'desktop_report_unread':
      only('instance')
      ctx.trusted.registry.report(own().id, wholeNumber(args, 'unread'), wholeNumber(args, 'mentions'))
      countsChanged(ctx)
      return null
    case 'desktop_set_badge':
      only('instance')
      ctx.trusted.registry.reportBadge(own().id, wholeNumber(args, 'count'))
      countsChanged(ctx)
      return null
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
  handle('mnema:invoke', (event) => caller(ctx, event), (from, command, args) => run(ctx, from, command, args))
  ipcMain.on('mnema:identity', (event) => {
    event.returnValue = identity(ctx, event)
  })
}
