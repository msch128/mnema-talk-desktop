import { closeSync, fsyncSync, mkdirSync, openSync, renameSync, rmSync, writeSync } from 'node:fs'
import { dirname } from 'node:path'

export interface AtomicOptions {
  /** File mode, 0600 by default: everything written here is private to the user. */
  mode?: number
  /** Create the parent directory with this mode when given. */
  dirMode?: number
  /** Flush the file to disk before the rename. */
  sync?: boolean
}

/**
 * Writes next to the target and renames over it, so a reader sees the old file
 * or the whole new one. The temporary file is removed again when the write
 * fails (a full disk must not leave a half-written package behind); the error
 * is passed on.
 */
export function writeFileAtomic(path: string, bytes: Uint8Array, options: AtomicOptions = {}): void {
  if (options.dirMode !== undefined) mkdirSync(dirname(path), { recursive: true, mode: options.dirMode })
  const partial = `${path}.partial`
  try {
    const fd = openSync(partial, 'w', options.mode ?? 0o600)
    try {
      writeSync(fd, bytes)
      if (options.sync) fsyncSync(fd)
    } finally {
      closeSync(fd)
    }
    renameSync(partial, path)
  } catch (error) {
    rmSync(partial, { force: true })
    throw error
  }
}
