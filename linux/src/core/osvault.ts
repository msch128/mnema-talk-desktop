/**
 * The vault over the operating system's secret service, written against a
 * small `SecretStorage` (Electron's `safeStorage` satisfies it) so that its
 * failure modes can be tested without Electron. See `main/vault.ts`.
 */
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Vault } from './cookies.ts'

/** The part of Electron's `safeStorage` the vault uses. */
export interface SecretStorage {
  isEncryptionAvailable(): boolean
  getSelectedStorageBackend(): string
  encryptString(plain: string): Buffer
  decryptString(encrypted: Buffer): string
}

const ACCOUNT = /^[0-9a-f]{16}$/

/** Whether the storage is backed by a real secret service (on Linux, Chromium's `basic_text` fixed password is not one). */
export function vaultAvailable(storage: SecretStorage, env: NodeJS.ProcessEnv, platform: NodeJS.Platform): boolean {
  // For tests and for anyone who wants the session in the private file instead of the secret service.
  if (env.MNEMA_DESKTOP_NO_VAULT) return false
  try {
    if (!storage.isEncryptionAvailable()) return false
    return platform !== 'linux' || storage.getSelectedStorageBackend() !== 'basic_text'
  } catch {
    return false
  }
}

const GNOME_LIKE = new Set(['GNOME', 'UNITY', 'CINNAMON', 'X-CINNAMON', 'XFCE', 'MATE', 'PANTHEON', 'BUDGIE', 'DEEPIN', 'LXQT', 'COSMIC'])

/**
 * Chromium picks its password store from the desktop environment and falls
 * back to a plaintext one on window-manager-only sessions (sway, i3, Hyprland)
 * even where a secret service runs. For such a session this names the store to
 * ask for; null leaves the choice to Chromium (a desktop it knows, or the user
 * passed `--password-store` themselves).
 */
export function passwordStoreSwitch(env: NodeJS.ProcessEnv, hasSwitch: (name: string) => boolean): string | null {
  if (hasSwitch('password-store')) return null
  const desktops = (env.XDG_CURRENT_DESKTOP ?? '').toUpperCase().split(':').filter((d) => d !== '')
  if (desktops.some((d) => d === 'KDE' || GNOME_LIKE.has(d))) return null
  return 'gnome-libsecret'
}

/** A vault over the storage, files under `<dir>/vault`. */
export function createVault(dir: string, storage: SecretStorage): Vault {
  const file = (account: string) => (ACCOUNT.test(account) ? join(dir, 'vault', `${account}.bin`) : null)
  return {
    load(account) {
      const path = file(account)
      if (!path) return null
      try {
        return Buffer.from(storage.decryptString(readFileSync(path)), 'utf8')
      } catch {
        return null
      }
    },
    save(account, secret) {
      const path = file(account)
      if (!path) return false
      const partial = `${path}.partial`
      try {
        mkdirSync(join(dir, 'vault'), { recursive: true, mode: 0o700 })
        writeFileSync(partial, storage.encryptString(secret.toString('utf8')), { mode: 0o600 })
        renameSync(partial, path)
        return true
      } catch {
        // No encrypted blob is left behind by a failed save.
        try { rmSync(partial, { force: true }) } catch { /* nothing more to do */ }
        return false
      }
    },
    remove(account) {
      const path = file(account)
      if (!path) return
      // Removing a server must go on even if a file cannot be deleted right now.
      for (const target of [path, `${path}.partial`]) {
        try { rmSync(target, { force: true }) } catch { /* the entry is unreadable without its key anyway */ }
      }
    },
  }
}
