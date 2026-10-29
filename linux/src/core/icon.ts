/**
 * The icon a server's owner sets for the community, shown on the rail. It comes
 * from the server itself and from nowhere else (`GET /api/community/icon`, over
 * the same HTTPS path as `/api/health`, no redirects), is checked here (png or
 * webp, bounded in bytes and pixels, well formed, never svg) and rests in the
 * data directory next to the other per-server data. Shared in its rules with the
 * Tauri host through `test-vectors/rail-icon.json`.
 */
import { createHash } from 'node:crypto'
import { mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { crc32 } from 'node:zlib'
import { writeFileAtomic } from './atomic.ts'
import { parseStrictJson } from './json.ts'
import { readBody, send } from './http.ts'
import { isLowerHex } from './encoding.ts'

/** What a stored icon may weigh: one mebibyte. */
export const MAX_ICON_BYTES = 1024 * 1024
/** What it may measure each way. */
export const MAX_ICON_SIDE = 2048
const ETAG_MAX = 256
export const ICON_PATH = 'api/community/icon'

export type IconKind = 'png' | 'webp'
export interface IconInfo { kind: IconKind, width: number, height: number }

const MIME: Record<IconKind, string> = { png: 'image/png', webp: 'image/webp' }

function inside(width: number, height: number): boolean {
  return width >= 1 && height >= 1 && width <= MAX_ICON_SIDE && height <= MAX_ICON_SIDE
}

function png(bytes: Uint8Array): IconInfo | null {
  const view = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.length)
  if (view.length < 8 || !view.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) return null
  let at = 8
  let size: { width: number, height: number } | null = null
  let data = false
  for (let first = true; ; first = false) {
    if (at + 12 > view.length) return null
    const length = view.readUInt32BE(at)
    const type = view.toString('latin1', at + 4, at + 8)
    const end = at + 8 + length
    if (end + 4 > view.length) return null
    if (crc32(view.subarray(at + 4, end)) !== view.readUInt32BE(end)) return null
    if (first) {
      if (type !== 'IHDR' || length !== 13) return null
      size = { width: view.readUInt32BE(at + 8), height: view.readUInt32BE(at + 12) }
      if (![0, 2, 3, 4, 6].includes(view[at + 17] as number)) return null
    } else if (type === 'IDAT') {
      data = true
    } else if (type === 'IEND') {
      return length === 0 && data && end + 4 === view.length && size && inside(size.width, size.height) ? { kind: 'png', ...size } : null
    }
    at = end + 4
  }
}

function webp(bytes: Uint8Array): IconInfo | null {
  const view = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.length)
  if (view.length < 20 || view.toString('latin1', 0, 4) !== 'RIFF' || view.toString('latin1', 8, 12) !== 'WEBP') return null
  if (view.readUInt32LE(4) !== view.length - 8) return null
  let at = 12
  let canvas: { width: number, height: number } | null = null
  let picture: { width: number, height: number } | null = null
  for (let first = true; at < view.length; first = false) {
    if (at + 8 > view.length) return null
    const type = view.toString('latin1', at, at + 4)
    const length = view.readUInt32LE(at + 4)
    const body = at + 8
    const next = body + length + (length & 1)
    if (next > view.length) return null
    if (first && type !== 'VP8X' && type !== 'VP8 ' && type !== 'VP8L') return null
    if (type === 'VP8X') {
      if (!first || length < 10) return null
      canvas = { width: 1 + view.readUIntLE(body + 4, 3), height: 1 + view.readUIntLE(body + 7, 3) }
    } else if (type === 'VP8 ') {
      if (length < 10 || view[body + 3] !== 0x9d || view[body + 4] !== 0x01 || view[body + 5] !== 0x2a) return null
      picture ??= { width: view.readUInt16LE(body + 6) & 0x3fff, height: view.readUInt16LE(body + 8) & 0x3fff }
    } else if (type === 'VP8L') {
      if (length < 5 || view[body] !== 0x2f) return null
      const bits = view.readUInt32LE(body + 1)
      picture ??= { width: 1 + (bits & 0x3fff), height: 1 + ((bits >>> 14) & 0x3fff) }
    }
    at = next
  }
  const size = canvas ?? picture
  return picture && size && inside(size.width, size.height) ? { kind: 'webp', ...size } : null
}

/** What these bytes are, if they are an icon the rail may show; `null` otherwise. The content type must agree with the bytes. */
export function checkIcon(bytes: Uint8Array, contentType: string | null | undefined): IconInfo | null {
  if (bytes.length === 0 || bytes.length > MAX_ICON_BYTES || !contentType) return null
  const declared = contentType.split(';')[0]?.trim().toLowerCase()
  return declared === MIME.png ? png(bytes) : declared === MIME.webp ? webp(bytes) : null
}

/** Names a version of an icon: the first eight bytes of its SHA-256. */
export function iconToken(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex').slice(0, 16)
}

/**
 * Whether to ask for the icon at all: the health answer says `community_icon`
 * as a flag or a version. Switched off (false, null, empty, 0) means none; on, or
 * absent (an older server: one cheap request, a 404 clears), means ask.
 */
export function iconWanted(health: Uint8Array): boolean {
  let parsed: unknown
  try { parsed = parseStrictJson(health) } catch { return true }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed) || !('community_icon' in parsed)) return true
  const value = (parsed as Record<string, unknown>)['community_icon']
  return !(value === false || value === null || value === '' || value === 0)
}

export type Fetched =
  | { status: 'changed', bytes: Buffer, kind: IconKind, etag: string | null }
  | { status: 'same' | 'none' | 'failed' }

/** One conditional request for the icon of the server at `origin`. Redirects are failures; nothing is read past the limit. */
export async function fetchIcon(origin: URL, etag: string | null, timeoutMs = 10_000): Promise<Fetched> {
  try {
    const response = await send({
      method: 'GET',
      url: new URL(ICON_PATH, origin),
      headers: { accept: 'image/png, image/webp', ...(etag ? { 'if-none-match': etag } : {}) },
      connectTimeoutMs: timeoutMs,
      totalTimeoutMs: timeoutMs,
    })
    if (response.status === 304) { response.destroy(); return { status: 'same' } }
    if (response.status === 404 || response.status === 410) { response.destroy(); return { status: 'none' } }
    if (response.status !== 200 || (response.contentLength !== null && response.contentLength > MAX_ICON_BYTES)) { response.destroy(); return { status: 'failed' } }
    const body = await readBody(response, MAX_ICON_BYTES)
    const type = response.headers['content-type']
    const info = body ? checkIcon(body, Array.isArray(type) ? type[0] : type) : null
    if (!body || !info) return { status: 'failed' }
    const tag = response.headers['etag']
    const text = Array.isArray(tag) ? tag[0] : tag
    return { status: 'changed', bytes: body, kind: info.kind, etag: text !== undefined && text.length <= ETAG_MAX && !/\p{Cc}/u.test(text) ? text : null }
  } catch {
    return { status: 'failed' }
  }
}

/** The icons at rest, one per server: `<id>.png` or `<id>.webp` and its `<id>.etag`. Without a directory nothing is kept. */
export class IconCache {
  private readonly dir: string | null

  constructor(dataDir: string | null) {
    this.dir = dataDir ? join(dataDir, 'icons') : null
  }

  private file(id: string, kind: IconKind | 'etag'): string | null {
    return this.dir && isLowerHex(id, 16) ? join(this.dir, `${id}.${kind}`) : null
  }

  /** The stored icon, checked again as it is read. */
  read(id: string): { bytes: Buffer, kind: IconKind } | null {
    for (const kind of ['png', 'webp'] as const) {
      const file = this.file(id, kind)
      if (!file) return null
      try {
        const bytes = readFileSync(file)
        const info = checkIcon(bytes, MIME[kind])
        if (info) return { bytes, kind }
      } catch { /* not there, or not readable */ }
    }
    return null
  }

  etag(id: string): string | null {
    const file = this.file(id, 'etag')
    if (!file || !this.read(id)) return null
    try {
      const text = readFileSync(file, 'utf8')
      return text !== '' && text.length <= ETAG_MAX ? text : null
    } catch {
      return null
    }
  }

  token(id: string): string | null {
    const icon = this.read(id)
    return icon ? iconToken(icon.bytes) : null
  }

  /** `data:image/...;base64,...` for the rail's `<img>`, or null. */
  dataUrl(id: string): string | null {
    const icon = this.read(id)
    return icon ? `data:${MIME[icon.kind]};base64,${icon.bytes.toString('base64')}` : null
  }

  store(id: string, bytes: Buffer, kind: IconKind, etag: string | null): void {
    const file = this.file(id, kind)
    const other = this.file(id, kind === 'png' ? 'webp' : 'png')
    const tag = this.file(id, 'etag')
    if (!file || !other || !tag || !this.dir) return
    mkdirSync(this.dir, { recursive: true, mode: 0o700 })
    writeFileAtomic(file, bytes, { dirMode: 0o700 })
    rmSync(other, { force: true })
    if (etag) writeFileAtomic(tag, Buffer.from(etag), { dirMode: 0o700 })
    else rmSync(tag, { force: true })
  }

  clear(id: string): void {
    for (const kind of ['png', 'webp', 'etag'] as const) {
      const file = this.file(id, kind)
      if (file) rmSync(file, { force: true })
    }
  }

  /** Removes the icons of servers that are not in the list. */
  sweep(keep: ReadonlySet<string>): void {
    if (!this.dir) return
    let names: string[]
    try { names = readdirSync(this.dir) } catch { return }
    for (const name of names) {
      const id = name.slice(0, name.lastIndexOf('.'))
      if (!keep.has(id)) rmSync(join(this.dir, name), { force: true })
    }
  }
}

/** Asks the server for its icon and keeps the answer. Returns whether what the rail shows changed. */
export async function refreshIcon(cache: IconCache, id: string, origin: URL, wanted: boolean): Promise<boolean> {
  const before = cache.token(id)
  if (!wanted) {
    cache.clear(id)
    return before !== null
  }
  const got = await fetchIcon(origin, cache.etag(id))
  try {
    if (got.status === 'none') cache.clear(id)
    else if (got.status === 'changed') cache.store(id, got.bytes, got.kind, got.etag)
  } catch { /* the icon is a convenience; the rail keeps what it had */ }
  return cache.token(id) !== before
}
