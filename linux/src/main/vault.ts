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
 * 0600 file instead (see `core/cookies.ts`). The logic lives in `core/osvault.ts`.
 */
import { safeStorage } from 'electron'
import type { Vault } from '../core/cookies.ts'
import { createVault, vaultAvailable } from '../core/osvault.ts'

export function osVaultAvailable(): boolean {
  return vaultAvailable(safeStorage, process.env, process.platform)
}

/** A vault over the secret service, or null where there is none. */
export function osVault(dir: string): Vault | null {
  return osVaultAvailable() ? createVault(dir, safeStorage) : null
}
