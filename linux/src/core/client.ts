/**
 * The pure rules of the trusted client: which instance addresses are
 * selectable, how `/api/health` is read, which links may leave the app and
 * which paths are routes of the bundled app. Ports of the matching functions in
 * `experimental/app/src/web_client.rs` and `local_route.rs`; the shared vectors
 * must pass in both.
 */
import { asU32, parseStrictJson } from './json.ts'
import { type ServerInfo } from './store.ts'
import { parseVersion } from './version.ts'

function isLoopback(url: URL): boolean {
  const host = url.hostname
  return host.toLowerCase() === 'localhost' || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host) || host === '[::1]'
}

/** One layer of quotes or angle brackets, as a link arrives from a chat message. */
function unwrapPaste(input: string): string {
  const trimmed = input.trim()
  if (trimmed.length >= 2) {
    const open = trimmed[0]
    const close = trimmed[trimmed.length - 1]
    if ((open === '<' && close === '>') || (open === '"' && close === '"') || (open === "'" && close === "'")) return trimmed.slice(1, -1).trim()
  }
  return trimmed
}

/** The pasted text as a URL, before the page (path, query, fragment) is removed. */
function prepared(input: string): URL {
  if (Buffer.byteLength(input) > 2048 || [...input].some((c) => /\p{Cc}/u.test(c))) throw new Error('Enter an HTTPS instance address.')
  const pasted = unwrapPaste(input)
  const raw = pasted.includes('://') ? pasted : `https://${pasted}`
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
  return url
}

/**
 * The address of an instance. A pasted invite link keeps the host and an
 * explicit port; the path, query and fragment are the page, not the server.
 * Plain http on another machine becomes https. This machine stays http.
 */
export function parseAddress(input: string): URL {
  const url = prepared(input)
  if (url.protocol === 'http:' && !isLoopback(url)) url.protocol = 'https:'
  url.pathname = '/'
  url.search = ''
  url.hash = ''
  return url
}

/** An invite code as the web app accepts it: letters, digits, "_" and "-", 6 to 64. */
function inviteSegment(value: string | undefined): boolean {
  return value !== undefined && /^[A-Za-z0-9_-]{6,64}$/.test(value)
}

function inviteToken(value: string | null): string | null {
  if (value === null) return null
  const code = value.trim().replace(/[.,;)]+$/u, '')
  return inviteSegment(code) ? code : null
}

function pathInvite(pathname: string): string | null {
  const match = /^\/invite\/([^/]+)\/?$/.exec(pathname)
  return match?.[1] ? inviteToken(match[1]) : null
}

function queryInvite(params: URLSearchParams): string | null {
  return inviteToken(params.get('invite'))
}

function fragmentInvite(hash: string): string | null {
  const query = hash.split('?')[1]
  return query === undefined ? null : inviteToken(new URLSearchParams(query).get('invite'))
}

/**
 * The invite code in a pasted link: `/invite/{code}`, `?invite=`, or the same
 * pair in the fragment. Anything else is not an invite. Connecting still works.
 */
export function inviteCode(input: string): string | null {
  try {
    const url = prepared(input)
    return pathInvite(url.pathname) ?? queryInvite(url.searchParams) ?? fragmentInvite(url.hash)
  } catch {
    return null
  }
}

export const UPDATE_NEEDED = 'This instance needs an update for the desktop client.'
export const NOT_COMPATIBLE = 'This instance is not compatible with this desktop client.'
/** Sent to the connect page, which translates it by exact text (`test-vectors/host-errors.json`). */
export const SELECTOR_ONLY = 'Only the instance selector may open an instance.'

/** `GET /api/health`: what the server says about itself. */
export function parseHealth(bytes: Uint8Array): ServerInfo {
  let health: unknown
  try {
    health = parseStrictJson(bytes)
  } catch {
    throw new Error(UPDATE_NEEDED)
  }
  if (typeof health !== 'object' || health === null || Array.isArray(health)) throw new Error(UPDATE_NEEDED)
  const { status, version, web_client_api: api } = health as Record<string, unknown>
  const apiOk = api === undefined || api === null || asU32(api) !== null
  if (typeof status !== 'string' || (version !== undefined && typeof version !== 'string') || !apiOk) throw new Error(UPDATE_NEEDED)
  if (status !== 'ok') throw new Error(NOT_COMPATIBLE)
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
 * Closed local SPA routes from the actual bundled Vue lib/router.ts and
 * lib/routePaths.ts. Admitting a local path grants no channel, admin, voice or
 * content permission.
 */
const SLUG = /^[a-z0-9-]{1,80}$/
const USERNAME = /^[A-Za-z0-9._~-]{1,64}$/

export function appPath(path: string): boolean {
  if (['', '/', '/index.html', '/login'].includes(path)) return true
  if (path.length > 200) return false
  const parts = path.split('/')
  if (parts[0] !== '') return false
  const [, first, second, third, fourth, fifth] = parts
  const id = (value: string | undefined) => value !== undefined && isId(value)
  const slug = (value: string | undefined) => value !== undefined && SLUG.test(value)
  const username = (value: string | undefined) => value !== undefined && USERNAME.test(value)
  const tail = (kind: string | undefined, value: string | undefined) => (kind === 'm' || kind === 't') && id(value)
  switch (parts.length) {
    case 2: return first === 'admin'
    case 3:
      if (first === 'invite') return inviteSegment(second)
      if (first === 'admin') return ['users', 'channels', 'invites', 'media', 'system'].includes(second as string)
      if (first === 'dm') return username(second)
      return (first === 'c' || first === 'v') && slug(second)
    case 4:
      if (first === 'invite') return third === '' && inviteSegment(second)
      if (first === 'dm') return second === 'g' && slug(third)
      return first === 'v' && slug(second) && third === 'chat'
    case 5:
      if (first === 'dm') return username(second) && tail(third, fourth)
      return first === 'c' && slug(second) && ((third === 'p' && slug(fourth)) || tail(third, fourth))
    case 6:
      if (first === 'dm') return second === 'g' && slug(third) && tail(fourth, fifth)
      return first === 'v' && slug(second) && third === 'chat' && fourth === 'm' && id(fifth)
    default: return false
  }
}
