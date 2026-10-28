/**
 * Finding and downloading newer signed bundles from the GitHub releases of
 * mnema-talk (a port of `experimental/app/src/web_updates.rs`). Where bytes
 * come from decides nothing: a download is installed only if
 * `Store.install` verifies it against the keys in force.
 */
import { LATEST_RELEASE_URL, TRUST_SIG_URL, TRUST_URL } from './pinned.ts'
import { strictJson } from './encoding.ts'
import { MAX_STATEMENT } from './trust.ts'
import { type HttpResponse, readBody, send } from './http.ts'
import { type TrustedKey } from './bundle.ts'
import { type Store, installCurrent } from './store.ts'
import { type Version, compareVersions, formatVersion, parseVersion } from './version.ts'

const MAX_RELEASE_JSON = 1 << 20
const MAX_DOWNLOAD = 96 << 20
const MAX_REDIRECTS = 5

export function updatesDisabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const value = env.MNEMA_DESKTOP_NO_UPDATE_CHECK
  return value !== undefined && value !== '' && value !== '0'
}

/** The newest release that carries a bundle: its version and download URL. */
export function bundleAsset(json: Uint8Array, allowHttp = false): { version: Version, url: URL } | null {
  let release: unknown
  try {
    release = strictJson(json)
  } catch {
    return null
  }
  if (typeof release !== 'object' || release === null) return null
  const { tag_name: tag, assets } = release as { tag_name?: unknown, assets?: unknown }
  if (typeof tag !== 'string' || !tag.startsWith('v')) return null
  const version = parseVersion(tag.slice(1))
  if (!version) return null
  const wanted = `mnema-web-${formatVersion(version)}.zip`
  if (assets !== undefined && !Array.isArray(assets)) return null
  // A release document with any malformed asset is refused as a whole, as serde does.
  const listed: { name: string, href: string, size: number }[] = []
  for (const asset of (assets ?? []) as unknown[]) {
    if (typeof asset !== 'object' || asset === null) return null
    const { name, browser_download_url: href, size = 0 } = asset as { name?: unknown, browser_download_url?: unknown, size?: unknown }
    if (typeof name !== 'string' || typeof href !== 'string' || typeof size !== 'number' || !Number.isSafeInteger(size) || size < 0) return null
    listed.push({ name, href, size })
  }
  const asset = listed.find((a) => a.name === wanted && a.size <= MAX_DOWNLOAD)
  if (!asset) return null
  let url: URL
  try {
    url = new URL(asset.href)
  } catch {
    return null
  }
  const scheme = url.protocol === 'https:' || (allowHttp && url.protocol === 'http:')
  return scheme && url.username === '' && url.password === '' ? { version, url } : null
}

export interface FetchOptions {
  /** Total time for one request, ms. */
  timeoutMs?: number
  /** Called with the bytes received so far and the declared total. */
  progress?: (received: number, total: number | null) => void
  /** Scheme allowed after a redirect (tests use loopback HTTP). */
  allowHttp?: boolean
}

/** GET with at most five redirects, HTTPS only (downloads are redirected to GitHub's asset hosts). */
export async function fetchLimited(url: URL, accept: string, limit: number, options: FetchOptions = {}): Promise<Buffer> {
  if (url.protocol !== 'https:' && !(options.allowHttp && url.protocol === 'http:')) throw new Error('unsupported scheme')
  let current = url
  for (let hop = 0; hop <= MAX_REDIRECTS; hop++) {
    let response: HttpResponse
    try {
      response = await send({
        method: 'GET',
        url: current,
        headers: { accept, 'user-agent': 'mnema-desktop' },
        connectTimeoutMs: 10_000,
        totalTimeoutMs: options.timeoutMs ?? 120_000,
      })
    } catch {
      throw new Error('unreachable')
    }
    if (response.status >= 300 && response.status < 400 && response.headers.location) {
      response.destroy()
      let next: URL
      try {
        next = new URL(response.headers.location, current)
      } catch {
        throw new Error('status 302')
      }
      if (next.protocol !== 'https:' && !(options.allowHttp && next.protocol === 'http:')) throw new Error(`status ${response.status}`)
      current = next
      continue
    }
    if (response.status < 200 || response.status >= 300) {
      response.destroy()
      throw new Error(`status ${response.status}`)
    }
    const body = await readBody(response, limit, options.progress)
    if (!body) throw new Error('too large')
    return body
  }
  throw new Error('too many redirects')
}

/** Where to look; tests point it at a loopback server. */
export interface Sources {
  trust?: URL
  trustSig?: URL
  latest?: URL
  fetch?: FetchOptions
}

/** The current trust statement and its signature. Unverified: the caller hands both to `TrustState.accept`, which checks the signature first. */
export async function fetchTrust(sources: Sources = {}): Promise<{ doc: Buffer, sig: Buffer }> {
  const doc = await fetchLimited(sources.trust ?? new URL(TRUST_URL), 'application/octet-stream', MAX_STATEMENT, sources.fetch)
  const sig = await fetchLimited(sources.trustSig ?? new URL(TRUST_SIG_URL), 'application/octet-stream', MAX_STATEMENT, sources.fetch)
  return { doc, sig }
}

/** Looks for a release newer than everything held, downloads it, verifies it and installs it. `version` when a new bundle was installed. */
export async function checkAndInstall(store: Store, keys: () => readonly TrustedKey[], sources: Sources = {}): Promise<Version | null> {
  if (updatesDisabled()) return null
  const release = await fetchLimited(sources.latest ?? new URL(LATEST_RELEASE_URL), 'application/vnd.github+json', MAX_RELEASE_JSON, sources.fetch)
  const found = bundleAsset(release, sources.fetch?.allowHttp === true)
  if (!found) return null
  if (store.held().some((h) => compareVersions(h.bundle.version, found.version) >= 0)) return null
  const bytes = await fetchLimited(found.url, 'application/octet-stream', MAX_DOWNLOAD, sources.fetch)
  return installCurrent(store, keys, bytes)
}
