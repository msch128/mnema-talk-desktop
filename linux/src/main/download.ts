/**
 * Downloads, as the person sees them. A click on a download link of the app's
 * own API reaches the host over IPC (`desktop_download`): the host fetches the
 * file with the server's session, asks where to put it, streams it there and
 * says what happened. A download Chromium starts itself (a blob, say) gets the
 * same feedback when it ends. Nothing ends without either the folder opening or
 * an error dialog.
 */
import { homedir } from 'node:os'
import { basename, dirname } from 'node:path'
import { Notification, app, dialog, shell, type DownloadItem } from 'electron'
import { INTERRUPTED, type DownloadRequest, openDownload } from '../core/download.ts'
import { inFolder } from '../core/downloadname.ts'
import { TITLE } from '../core/title.ts'
import type { Context } from './context.ts'
import { errorBox } from './errorbox.ts'

type Session = NonNullable<ReturnType<Context['trusted']['session']>>

/** Servers with a download under way: a second one waits for the person to try again. */
const running = new Set<string>()

/**
 * A folder for the end-to-end run: the file goes there without a dialog or a
 * file manager. Honoured only in a run that may be remote-debugged, which the
 * tests turn on and a packaged client refuses otherwise.
 */
function testFolder(): string | null {
  const folder = process.env.MNEMA_DESKTOP_DOWNLOAD_DIR ?? ''
  return folder !== '' && process.env.MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING === '1' ? folder : null
}

function downloadsFolder(): string {
  try {
    return app.getPath('downloads')
  } catch {
    return homedir()
  }
}

async function problem(message: string): Promise<void> {
  if (testFolder() !== null) console.error(`[download] ${message}`)
  else await errorBox(message)
}

function saved(path: string): void {
  if (testFolder() !== null) return
  shell.showItemInFolder(path)
  if (Notification.isSupported()) new Notification({ title: TITLE, body: `Saved ${basename(path)} in ${dirname(path)}`, silent: true }).show()
}

/** Where the file goes: the person's choice in the save dialog (theirs, not limited), or null for none. */
async function destination(ctx: Context, name: string): Promise<string | null> {
  const folder = testFolder()
  if (folder !== null) return inFolder(folder, name)
  const parent = ctx.shell.window
  const options = { title: TITLE, defaultPath: inFolder(downloadsFolder(), name) }
  const { canceled, filePath } = parent && !parent.isDestroyed() ? await dialog.showSaveDialog(parent, options) : await dialog.showSaveDialog(options)
  return canceled || filePath === '' ? null : filePath
}

/** Fetches, asks, writes and reports; never throws. */
export async function download(ctx: Context, session: Session, request: DownloadRequest): Promise<null> {
  if (running.has(session.id)) {
    await problem('Another download is still in progress. Try again when it has finished.')
    return null
  }
  running.add(session.id)
  try {
    const opened = await openDownload(session.instance, request.path, request.name)
    if ('error' in opened) {
      await problem(opened.error)
      return null
    }
    const { pending } = opened
    const dest = await destination(ctx, pending.name).catch(() => null)
    if (dest === null) {
      pending.abandon()
      return null
    }
    const result = await pending.save(dest)
    if (result.ok) saved(dest)
    else await problem(result.error)
  } catch {
    await problem(INTERRUPTED)
  } finally {
    running.delete(session.id)
  }
  return null
}

/** A download Chromium handles itself: when it ends the folder opens, or the person is told it failed. A cancelled one was the person's own doing. */
export function reportWhenDone(item: DownloadItem): void {
  item.once('done', (_event, state) => {
    if (state === 'completed') saved(item.getSavePath())
    else if (state === 'interrupted') void errorBox(INTERRUPTED)
  })
}
