/**
 * What the store keeps on disk next to the cache: the recorded floors and marks,
 * and the verification of the cached bundle files.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { MAX_ZIP, type Bundle, type TrustedKey, BundleError, verifyBundle } from './bundle.ts'
import type { Held } from './store.ts'
import { type Version, compareVersions, parseVersion } from './version.ts'

const MAX_CACHED_FILES = 16

export interface PersistedState {
  /** Highest `security_floor` each signing key ever declared (key id). */
  floors: Record<string, string>
  /** Highest bundle version ever verified per `web_client_api` and signing key. */
  marks: Record<string, Record<string, string>>
  last_check: number
}

export function readVersions(raw: unknown): Map<string, Version> {
  const out = new Map<string, Version>()
  if (typeof raw === 'object' && raw !== null) {
    for (const [key, value] of Object.entries(raw)) {
      const v = typeof value === 'string' ? parseVersion(value) : null
      if (v) out.set(key, v)
    }
  }
  return out
}

export function verifyFile(path: string, keys: readonly TrustedKey[]): Bundle {
  let size: number
  try {
    size = statSync(path).size
  } catch {
    throw new BundleError('unreadable')
  }
  if (size > MAX_ZIP) throw new BundleError('unreadable')
  let bytes: Buffer
  try {
    bytes = readFileSync(path)
  } catch {
    throw new BundleError('unreadable')
  }
  return verifyBundle(bytes, keys)
}

export function cachedFiles(dir: string | null): string[] {
  if (!dir) return []
  let names: string[]
  try {
    names = readdirSync(dir)
  } catch {
    return []
  }
  const files = names
    .filter((name) => name.startsWith('mnema-web-') && name.endsWith('.zip'))
    .filter((name) => { try { return statSync(join(dir, name)).isFile() } catch { return false } })
    .sort()
  return files.slice(0, MAX_CACHED_FILES)
}

/** The recorded state; a missing or damaged one is rebuilt from the held bundles. */
export function readState(dir: string | null): Partial<PersistedState> {
  if (!dir) return {}
  try {
    const parsed: unknown = JSON.parse(readFileSync(join(dir, 'state.json'), 'utf8'))
    if (typeof parsed === 'object' && parsed !== null) return parsed as Partial<PersistedState>
  } catch { /* rebuilt */ }
  return {}
}

/** The recorded marks per web client API; entries that are not a contract number are dropped. */
export function readMarks(raw: PersistedState['marks'] | undefined): Map<number, Map<string, Version>> {
  const marks = new Map<number, Map<string, Version>>()
  for (const [api, by] of Object.entries(raw ?? {})) {
    const n = Number(api)
    if (Number.isInteger(n) && n >= 1) marks.set(n, readVersions(by))
  }
  return marks
}

/**
 * Verifies the built-in bundle and every cached file. A bundle that fails
 * verification is named in `rejected`; a version held twice is kept once.
 */
export function verifyAll(dir: string | null, builtin: Uint8Array | null, keys: readonly TrustedKey[]): { verified: Held[], rejected: string[] } {
  const verified: Held[] = []
  const rejected: string[] = []
  if (builtin) {
    try {
      verified.push({ bundle: verifyBundle(builtin, keys), source: 'builtin' })
    } catch (error) {
      if (!(error instanceof BundleError)) throw error
      rejected.push('built-in')
    }
  }
  for (const name of cachedFiles(dir)) {
    try {
      const bundle = verifyFile(join(dir as string, name), keys)
      if (!verified.some((h) => compareVersions(h.bundle.version, bundle.version) === 0)) verified.push({ bundle, source: 'cache' })
    } catch (error) {
      if (!(error instanceof BundleError)) throw error
      rejected.push(name)
    }
  }
  return { verified, rejected }
}
