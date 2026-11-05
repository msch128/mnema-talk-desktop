/**
 * Taking a server away: everything this device holds of it is wiped, and the
 * storage of servers that are no longer in the list goes at the next start.
 */
import { existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { app, session } from 'electron'
import { orphanPartitions, wipeWithRetry } from '../core/wipe.ts'
import { LEGACY_PARTITION, serverPartition, type Context } from './context.ts'
import { toastsClosed } from './notify.ts'
import { changed, countsChanged, focusFront } from './shelllayout.ts'

function partitionDir(ctx: Context, partition: string): string {
  return join(ctx.trusted.dataDirectory() ?? app.getPath('userData'), 'Partitions', partition.replace(/^persist:/, ''))
}

/**
 * Removes a server's partition directory after its page stored what it stored
 * in the browser engine, in the background, and gives up if the server comes
 * back in the meantime.
 */
function wipeDirectory(ctx: Context, id: string, dir: string): void {
  wipeWithRetry(dir, { stop: () => ctx.trusted.registry.contains(id) })
}

/**
 * Takes a server out of the list and wipes everything this device holds of it:
 * its stored session (secret store entry and fallback file), its browser
 * storage, its sockets, its toasts and its attachment window (an in-memory session, so there is no directory of it).
 */
export async function removeServer(ctx: Context, id: string): Promise<void> {
  const { trusted } = ctx
  const partition = serverPartition(id)
  const slot = trusted.registry.remove(id)
  if (!slot) return
  if (slot.phase.kind === 'ready') {
    slot.phase.session.sockets.closeAll()
    slot.phase.session.instance.jar.forget()
  } else {
    trusted.forgetStoredSession(slot.origin)
  }
  trusted.icons.clear(id)
  toastsClosed(id, slot.generation)
  const attachment = ctx.shell.attachments.get(id)
  const stores = [attachment?.webContents.session]
  if (attachment && !attachment.isDestroyed()) attachment.close()
  const page = ctx.shell.pages.get(id)
  if (page) {
    stores.push(page.webContents.session)
    ctx.shell.pages.delete(id)
    ctx.shell.window?.contentView.removeChildView(page)
    page.webContents.close()
  }
  for (const ses of stores) {
    if (!ses) continue
    await ses.clearStorageData().catch(() => undefined)
    await ses.clearCache().catch(() => undefined)
  }
  wipeDirectory(ctx, id, partitionDir(ctx, partition))
  changed(ctx)
  countsChanged(ctx)
  focusFront(ctx)
}

/**
 * Storage of servers that are no longer in the list (a removal cut short by the
 * end of the program) goes at the next start. Only when the list was read in
 * full: a list that could not be (unreadable, a newer version, an address that
 * no longer holds up) says nothing about which servers are gone, and the end-to-end
 * state of the others lives in exactly these directories.
 */
export function sweepOrphans(ctx: Context): void {
  if (!ctx.trusted.registry.readInFull) return
  const root = join(ctx.trusted.dataDirectory() ?? app.getPath('userData'), 'Partitions')
  let names: string[]
  try { names = readdirSync(root) } catch { return }
  for (const name of orphanPartitions(names, new Set(ctx.trusted.registry.ids()))) wipeWithRetry(join(root, name))
}

/** Removes the storage of the single-server client: its data, then its directory. */
export async function wipeLegacyStorage(ctx: Context): Promise<void> {
  const ses = session.fromPartition(LEGACY_PARTITION)
  await ses.clearStorageData().catch(() => undefined)
  await ses.clearCache().catch(() => undefined)
  wipeWithRetry(partitionDir(ctx, LEGACY_PARTITION))
}

/** Whether the single-server client left its storage behind (it is gone once wiped, so this is true once). */
export function legacyStorageWaits(ctx: Context): boolean {
  const dir = partitionDir(ctx, LEGACY_PARTITION)
  return existsSync(dir)
}
