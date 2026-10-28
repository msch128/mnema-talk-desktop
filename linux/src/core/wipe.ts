/**
 * Removing what a server left on disk. Free of Electron so it can be tested.
 */
import { existsSync, rmSync } from 'node:fs'

/** Tries this often, this far apart: the engine may hold the files a moment after its session is cleared. */
const TRIES = 20
const RETRY_MS = 500

export interface WipeOptions {
  /** Checked before every try: the directory is left alone once this says true (the server came back). */
  stop?: () => boolean
  remove?: (dir: string) => void
  tries?: number
  retryMs?: number
}

/**
 * Removes a directory, then tries again in the background while it is still
 * there. A failure never throws: a directory that cannot be removed (read-only,
 * held by another process) is left and retried, never a reason to stop the
 * caller.
 */
export function wipeWithRetry(dir: string, options: WipeOptions = {}): void {
  const { stop = () => false, remove = (path: string) => rmSync(path, { recursive: true, force: true }), tries = TRIES, retryMs = RETRY_MS } = options
  let attempts = 0
  const attempt = () => {
    if (stop() || !existsSync(dir)) return
    try { remove(dir) } catch { /* tried again below */ }
    if (existsSync(dir) && ++attempts < tries) setTimeout(attempt, retryMs).unref()
  }
  attempt()
}

/** The partition directories of servers that are not in the list (`inst-<id>`); anything else in the folder is not ours to touch. */
export function orphanPartitions(names: readonly string[], known: ReadonlySet<string>): string[] {
  return names.filter((name) => {
    const match = /^inst-([0-9a-f]{16})$/.exec(name)
    return match !== null && !known.has(match[1] as string)
  })
}
