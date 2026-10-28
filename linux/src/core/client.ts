/**
 * The pure rules of the trusted client: which instance addresses are
 * selectable, how `/api/health` is read, which links may leave the app and
 * which paths are routes of the bundled app. Ports of the matching functions in
 * `experimental/app/src/web_client.rs` and `local_route.rs`; the shared vectors
 * must pass in both.
 */
import { type ServerInfo } from './store.ts'
import { parseVersion } from './version.ts'

function isLoopback(url: URL): boolean {
  const host = url.hostname
  return host.toLowerCase() === 'localhost' || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host) || host === '[::1]'
}

/**
 * The address of an instance. A pasted invite link keeps the host and an
 * explicit port; the path, query and fragment are the page, not the server.
 * Plain http on another machine becomes https. This machine stays http.
 */
export function parseAddress(input: string): URL {
  if (Buffer.byteLength(input) > 2048 || [...input].some((c) => /\p{Cc}/u.test(c))) throw new Error('Enter an HTTPS instance address.')
  const trimmed = input.trim()
  const raw = trimmed.includes('://') ? trimmed : `https://${trimmed}`
  if (raw.includes('\\') || /\s/u.test(raw)) throw new Error('Invalid instance address.')
  let url: URL
  try {
    url = new URL(raw)
  } catch {
    throw new Error('Invalid instance address.')
  }
  const authority = raw.slice(raw.indexOf('://') + 3)
  if (url.hostname === '') throw new Error('Invalid instance address.')
  if (url.username !== '' || url.password !== '' || authority.includes('@') || (url.protocol !== 'https:' && url.protocol !== 'http:')) {
    throw new Error('Use an HTTPS origin without credentials, a path or query.')
  }
  if (url.protocol === 'http:' && !isLoopback(url)) url.protocol = 'https:'
  url.pathname = '/'
  url.search = ''
  url.hash = ''
  return url
}

const UPDATE_NEEDED = 'This instance needs an update for the desktop client.'

/** `GET /api/health`: what the server says about itself. */
export function parseHealth(bytes: Uint8Array): ServerInfo {
  let health: unknown
  try {
    health = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
  } catch {
    throw new Error(UPDATE_NEEDED)
  }
  if (typeof health !== 'object' || health === null || Array.isArray(health)) throw new Error(UPDATE_NEEDED)
  const { status, version, web_client_api: api } = health as Record<string, unknown>
  const apiOk = api === undefined || api === null || (typeof api === 'number' && Number.isInteger(api) && api >= 0 && api <= 0xffffffff)
  if (typeof status !== 'string' || (version !== undefined && typeof version !== 'string') || !apiOk) throw new Error(UPDATE_NEEDED)
  if (status !== 'ok') throw new Error('This instance is not compatible with this desktop client.')
  if (typeof api !== 'number' || api < 1) throw new Error(UPDATE_NEEDED)
  return { version: parseVersion(version ?? ''), webClientApi: api }
}

/** Links that may leave the app, after a confirmation: HTTPS to a host, no credentials, bounded. */
export function externalAllowed(url: URL): boolean {
  return url.protocol === 'https:' && url.hostname !== '' && url.username === '' && url.password === '' && url.href.length <= 4096
}

function isId(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) && value !== '00000000-0000-0000-0000-000000000000'
}

/**
 * Closed local SPA routes from the actual bundled Vue lib/router.ts. Admitting
 * a local path grants no channel, admin, voice or content permission.
 */
export function appPath(path: string): boolean {
  if (['', '/', '/index.html', '/login'].includes(path)) return true
  if (path.length > 200) return false
  const parts = path.split('/')
  if (parts[0] !== '') return false
  const [, first, second, third, fourth, fifth] = parts
  const id = (value: string | undefined) => value !== undefined && isId(value)
  switch (parts.length) {
    case 2: return first === 'admin'
    case 3: return first === 'admin' ? ['users', 'channels', 'invites', 'media', 'system'].includes(second as string) : (first === 'c' || first === 'v') && id(second)
    case 4: return first === 'v' && id(second) && third === 'chat'
    case 5: return first === 'c' && id(second) && (third === 'm' || third === 't') && id(fourth)
    case 6: return first === 'v' && id(second) && third === 'chat' && fourth === 'm' && id(fifth)
    default: return false
  }
}
