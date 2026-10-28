/**
 * Where the session cookie rests: Electron's `safeStorage`, which on Linux
 * keeps its key in the desktop's secret service (libsecret: GNOME Keyring,
 * KWallet). The encrypted blob is a file next to the application data; without
 * the key it is noise, so a copied profile, a backup or a stray file listing no
 * longer carries a live session. A program running as the same user can still
 * ask the secret service.
 *
 * Where no secret service exists Chromium falls back to a fixed password
 * (`basic_text`): that is not a vault, so there is none, and the jar uses its
 * 0600 file instead (see `core/cookies.ts`).
 */
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { safeStorage } from 'electron'
import type { Vault } from '../core/cookies.ts'

const ACCOUNT = /^[0-9a-f]{16}$/

export function osVaultAvailable(): boolean {
  // For tests and for anyone who wants the session in the private file instead of the secret service.
  if (process.env.MNEMA_DESKTOP_NO_VAULT) return false
  try {
    if (!safeStorage.isEncryptionAvailable()) return false
    return process.platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text'
  } catch {
    return false
  }
}

/** A vault over the secret service, or null where there is none. */
export function osVault(dir: string): Vault | null {
  if (!osVaultAvailable()) return null
  const file = (account: string) => (ACCOUNT.test(account) ? join(dir, 'vault', `${account}.bin`) : null)
  return {
    load(account) {
      const path = file(account)
      if (!path) return null
      let blob: Buffer
      try {
        blob = readFileSync(path)
      } catch {
        return null
      }
      // A blob that will not decrypt (locked keyring) throws: the entry exists and must not count as absent.
      return Buffer.from(safeStorage.decryptString(blob), 'utf8')
    },
    save(account, secret) {
      const path = file(account)
      if (!path) return false
      try {
        mkdirSync(join(dir, 'vault'), { recursive: true, mode: 0o700 })
        const partial = `${path}.partial`
        writeFileSync(partial, safeStorage.encryptString(secret.toString('utf8')), { mode: 0o600 })
        renameSync(partial, path)
        return true
      } catch {
        return false
      }
    },
    remove(account) {
      const path = file(account)
      if (path) rmSync(path, { force: true })
    },
  }
}
