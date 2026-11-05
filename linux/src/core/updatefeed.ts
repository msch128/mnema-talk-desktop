/**
 * The pure rules of the signed self-update: which versions are offered, where a
 * package may come from, and how the release feed (the Tauri updater format,
 * shared with the Tauri client) is read. See `selfupdate.ts` for the whole.
 */
import { parseStrictJson } from './json.ts'
import { CHECK_INTERVAL_SECS } from './store.ts'
import { type Version, compareVersions, parseVersion } from './version.ts'

const NOTES_MAX = 2000

/** Whether the interval has passed. A clock set back counts as due. */
export function checkDue(lastCheck: number, now: number): boolean {
  return lastCheck > now || now - lastCheck >= CHECK_INTERVAL_SECS
}

/** The candidate version when it is strictly newer than `current`. Versions are plain `MAJOR.MINOR.PATCH`; anything else is not offered. */
export function offerable(current: string, candidate: string): Version | null {
  const next = parseVersion(candidate)
  const now = parseVersion(current)
  return next && now && compareVersions(next, now) > 0 ? next : null
}

/** A skipped version stays quiet; a newer one is offered again. */
export function isSkipped(skipped: string | null, candidate: string): boolean {
  return skipped === candidate
}

/**
 * The package is fetched from GitHub's release downloads over HTTPS (which
 * redirects to GitHub's asset hosts). The signature decides trust; this keeps
 * the request from going elsewhere in the first place.
 */
export function downloadUrlOk(url: URL): boolean {
  return url.protocol === 'https:' && url.hostname === 'github.com' && url.username === '' && url.password === '' && url.port === ''
}

/** The `{os}-{arch}` key of the feed for this machine. */
export function platformKey(platform: string = process.platform, arch: string = process.arch, deb = false): string | null {
  if (platform !== 'linux') return null
  const suffix = deb ? '-deb' : ''
  if (arch === 'x64') return `linux-x86_64${suffix}`
  if (arch === 'arm64') return `linux-aarch64${suffix}`
  return null
}

export interface Update {
  version: string
  notes: string | null
  signature: string
  url: URL
  /** The minisign public key this update must verify against (pinned, or listed by the trust statement). */
  key: string
}

/** Reads the feed. Tauri clients and this one share it; entries for other platforms are ignored. */
export function parseFeed(bytes: Uint8Array, platform: string): Omit<Update, 'key'> | null {
  let feed: unknown
  try {
    feed = parseStrictJson(bytes)
  } catch {
    return null
  }
  if (typeof feed !== 'object' || feed === null) return null
  const { version, notes, platforms } = feed as { version?: unknown, notes?: unknown, platforms?: unknown }
  if (typeof version !== 'string' || typeof platforms !== 'object' || platforms === null) return null
  const entry = (platforms as Record<string, unknown>)[platform]
  if (typeof entry !== 'object' || entry === null) return null
  const { signature, url } = entry as { signature?: unknown, url?: unknown }
  if (typeof signature !== 'string' || typeof url !== 'string') return null
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return null
  }
  return { version: version.replace(/^v/, ''), notes: trimNotes(typeof notes === 'string' ? notes : null), signature, url: parsed }
}

export function trimNotes(body: string | null): string | null {
  if (body === null) return null
  const kept = [...body].filter((c) => !/\p{Cc}/u.test(c) || c === '\n').slice(0, NOTES_MAX).join('')
  return kept.trim() === '' ? null : kept
}
