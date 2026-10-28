/**
 * The sessions of the pages. Every kind of page, and every server, gets its own
 * Electron session (a partition), and the `mnema:` handler of that session is
 * made for it alone: what a page may fetch does not depend on anything it
 * could claim. A server's page and its attachment window are bound to the id
 * of their server when the handler is made, so the page of one server can only
 * ever reach that server.
 */
import { session } from 'electron'
import { APP_SCHEME } from '../core/origin.ts'
import { attachmentPartition, serverPartition, type Context, type Role } from './context.ts'
import { secureSession } from './permissions.ts'
import { type RequestHandler, appHandler } from './protocol.ts'

/** Sets the protocol handler and the permissions of a partition, once. */
export function prepare(ctx: Context, partition: string, role: Role, handler: RequestHandler): void {
  if (ctx.prepared.has(partition)) return
  ctx.prepared.add(partition)
  const ses = session.fromPartition(partition)
  ses.protocol.handle(APP_SCHEME, handler)
  secureSession(ctx, ses, role)
}

/** The partition of a server's page, made ready. */
export function prepareServer(ctx: Context, id: string): string {
  const partition = serverPartition(id)
  prepare(ctx, partition, 'instance', appHandler('instance', ctx.trusted, ctx.debug, id))
  return partition
}

export function prepareAttachment(ctx: Context, id: string): string {
  const partition = attachmentPartition(id)
  prepare(ctx, partition, 'attachment', appHandler('attachment', ctx.trusted, ctx.debug, id))
  return partition
}
