import { chmodSync, existsSync, renameSync, rmSync, writeFileSync } from 'node:fs'

/**
 * Replaces the running AppImage with a verified package: the new file is
 * written next to it and renamed over it, so a failure leaves the installed
 * program untouched. A running AppImage keeps working after the rename (its
 * inode stays mapped until it exits).
 */
export function replaceAppImage(target: string, bytes: Uint8Array): void {
  if (bytes.length < 4 || bytes[0] !== 0x7f || bytes[1] !== 0x45 || bytes[2] !== 0x4c || bytes[3] !== 0x46) throw new Error('not an executable')
  if (!existsSync(target)) throw new Error('the AppImage is not where it was started from')
  const fresh = `${target}.new`
  try {
    writeFileSync(fresh, bytes, { mode: 0o755 })
    chmodSync(fresh, 0o755)
    renameSync(fresh, target)
  } catch (error) {
    rmSync(fresh, { force: true })
    throw error
  }
}
