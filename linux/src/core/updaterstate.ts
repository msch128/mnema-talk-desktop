/** What the self-updater remembers between runs: the last check, a skipped version and a kept package. */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { writeFileAtomic } from './atomic.ts'

export const STATE_FILE = 'native-update.json'
export const PENDING_FILE = 'native-update.pending'

export interface Pending { version: string }

export interface Persisted {
  last_check: number
  skipped: string | null
  pending: Pending | null
}

export function loadPersisted(dir: string | null): Persisted {
  const empty: Persisted = { last_check: 0, skipped: null, pending: null }
  if (!dir) return empty
  try {
    const raw: unknown = JSON.parse(readFileSync(join(dir, STATE_FILE), 'utf8'))
    if (typeof raw !== 'object' || raw === null) return empty
    const { last_check: last, skipped, pending } = raw as Record<string, unknown>
    const held = typeof pending === 'object' && pending !== null ? (pending as Record<string, unknown>) : null
    return {
      last_check: typeof last === 'number' && Number.isInteger(last) && last >= 0 ? last : 0,
      skipped: typeof skipped === 'string' ? skipped : null,
      pending: held && typeof held.version === 'string' ? { version: held.version } : null,
    }
  } catch {
    return empty
  }
}

export function writeAtomic(path: string, bytes: Uint8Array): void {
  writeFileAtomic(path, bytes, { dirMode: 0o700 })
}
