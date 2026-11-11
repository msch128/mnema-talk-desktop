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
  // Only the authority can carry credentials; an "@" in the path or query is page content.
  const authority = raw.slice(raw.indexOf('://') + 3).split(/[/?#]/u)[0] ?? ''
  if (url.hostname === '') throw new Error('Invalid instance address.')
  if (url.username !== '' || url.password !== '' || authority.includes('@') || (url.protocol !== 'https:' && url.protocol !== 'http:')) {
    throw new Error('Use a web address (https://…) without a user name or password.')
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

/**
 * What a `503` of `/api/health` says about a server that answers but reports a
 * dependency as down (object storage, for one: uploads and pictures fail then,
 * chat and Talks work). Such a server sends what its `200` carries with the
 * status `unavailable`. Null for every other body: an older server sends only
 * an error there. Port of `parse_degraded_health`.
 */
export function parseDegradedHealth(bytes: Uint8Array): ServerInfo | null {
  let health: unknown
  try {
    health = parseStrictJson(bytes)
  } catch {
    return null
  }
  if (typeof health !== 'object' || health === null || Array.isArray(health)) return null
  const { status, version, web_client_api: api } = health as Record<string, unknown>
  if (status !== 'unavailable' || typeof version !== 'string' || typeof api !== 'number' || !Number.isInteger(api) || asU32(api) === null || api < 1) return null
  return { version: parseVersion(version), webClientApi: api }
}

const MAX_NAME_CHARS = 80

/**
 * The name a server gives itself in `/api/health` (its Owner's community name,
 * else its host), or null. It is only ever shown as text, so control
 * characters become spaces and it is capped. Port of `parse_community_name`
 * in `experimental/app/src/web_client.rs`.
 */
export function parseCommunityName(bytes: Uint8Array): string | null {
  let health: unknown
  try {
    health = parseStrictJson(bytes)
  } catch {
    return null
  }
  if (typeof health !== 'object' || health === null || Array.isArray(health)) return null
  const raw = (health as Record<string, unknown>)['community_name']
  if (typeof raw !== 'string') return null
  // eslint-disable-next-line no-control-regex
  const name = Array.from(raw.replace(/[\u0000-\u001f\u007f-\u009f]/g, ' ').split(/\s+/).filter(Boolean).join(' ')).slice(0, MAX_NAME_CHARS).join('')
  return name === '' ? null : name
}

/** Links that may leave the app, after a confirmation: HTTPS to a host without credentials, or a mailto: address; bounded. */
export function externalAllowed(url: URL): boolean {
  if (url.protocol === 'https:') return url.hostname !== '' && url.username === '' && url.password === '' && url.href.length <= 4096
  // The web app links an e-mail address in a message as `mailto:`. No other scheme (tel:, file:, an app's own) leaves the app.
  // eslint-disable-next-line no-control-regex
  if (url.protocol === 'mailto:') return url.pathname !== '' && url.href.length <= 2048 && !/[\u0000-\u001f\u007f]/.test(url.href)
  return false
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
/** The tabs of the admin console and the sections of the settings dialog (web app lib/adminNav.ts, lib/settingsDialog.ts). */
const ADMIN_TABS = ['users', 'roles', 'channels', 'invites', 'onboarding', 'access', 'emoji', 'reports', 'automod', 'bans', 'bots', 'import', 'media', 'audit', 'system', 'status']
const SETTINGS_TABS = ['audio', 'app', 'signin', 'sessions', 'privacy', 'roles', 'data']
const USERNAME = /^[A-Za-z0-9._~-]{1,64}$/

type Part = string | undefined

const isIdPart = (value: Part) => value !== undefined && isId(value)
const isSlug = (value: Part) => value !== undefined && SLUG.test(value)
const isUsername = (value: Part) => value !== undefined && USERNAME.test(value)
/** `m` (a message) or `t` (a thread) followed by its id. */
const isTail = (kind: Part, value: Part) => (kind === 'm' || kind === 't') && isIdPart(value)

export function appPath(path: string): boolean {
  if (['', '/', '/index.html', '/login'].includes(path)) return true
  if (path.length > 200) return false
  const parts = path.split('/')
  if (parts[0] !== '') return false
  const [, first, second, third, fourth, fifth] = parts
  switch (parts.length) {
    case 2: return first === 'admin' || first === 'settings' || first === 'wiki'
    case 3: return twoSegments(first, second)
    case 4: return threeSegments(first, second, third)
    case 5: return fourSegments(first, second, third, fourth)
    case 6: return fiveSegments(first, second, third, fourth, fifth)
    default: return false
  }
}

function twoSegments(first: Part, second: Part): boolean {
  if (first === 'invite') return inviteSegment(second)
  if (first === 'admin') return ADMIN_TABS.includes(second as string)
  if (first === 'settings') return SETTINGS_TABS.includes(second as string)
  if (first === 'wiki') return isSlug(second)
  if (first === 'dm') return isUsername(second)
  return (first === 'c' || first === 'v') && isSlug(second)
}

function threeSegments(first: Part, second: Part, third: Part): boolean {
  if (first === 'invite') return third === '' && inviteSegment(second)
  if (first === 'dm') return second === 'g' && isSlug(third)
  // A wiki page's history or editor.
  if (first === 'wiki') return isSlug(second) && (third === 'history' || third === 'edit')
  return first === 'v' && isSlug(second) && third === 'chat'
}

function fourSegments(first: Part, second: Part, third: Part, fourth: Part): boolean {
  if (first === 'dm') return isUsername(second) && isTail(third, fourth)
  return first === 'c' && isSlug(second) && ((third === 'p' && isSlug(fourth)) || isTail(third, fourth))
}

function fiveSegments(first: Part, second: Part, third: Part, fourth: Part, fifth: Part): boolean {
  if (first === 'dm') return second === 'g' && isSlug(third) && isTail(fourth, fifth)
  return first === 'v' && isSlug(second) && third === 'chat' && fourth === 'm' && isIdPart(fifth)
}

/** What the confirmation shows before a link leaves the app: the whole address, so a mailto: cannot hide a cc, bcc or body. */
export function externalPrompt(url: URL): { message: string, detail: string } {
  return { message: url.protocol === 'mailto:' ? 'Write to this address in your mail program?' : 'Open this link in your browser?', detail: url.href }
}
