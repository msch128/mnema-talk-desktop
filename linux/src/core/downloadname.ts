/**
 * The name a download is saved under. The server names it (`Content-Disposition`),
 * the page may suggest one (the `download` attribute), the address is the last
 * resort; whatever is chosen is cut down to a plain file name, so no answer can
 * point the file at another folder. The cases are in `download_names` of `test-vectors/proxy.json`,
 * which the Rust host reads as well.
 */
import { basename, dirname, join, resolve } from 'node:path'

function isControl(code: number): boolean {
  return code < 0x20 || (code >= 0x7f && code <= 0x9f)
}

/**
 * The parameters of a `Content-Disposition` value by lower-case name, the first
 * of each. A value is a quoted string, taken as written (no escapes), or a
 * token up to the next `;`.
 */
function parameters(disposition: string): Map<string, string> {
  const found = new Map<string, string>()
  const start = disposition.indexOf(';')
  if (start < 0) return found
  for (const match of disposition.slice(start).matchAll(/;\s*([^=;]*?)\s*=\s*(?:"([^"]*)"|([^;]*))/g)) {
    const name = (match[1] as string).toLowerCase()
    if (!found.has(name)) found.set(name, match[2] ?? (match[3] as string).trim())
  }
  return found
}

/** `filename*` (RFC 5987): `UTF-8''` and percent-encoded bytes; any other charset or broken encoding is ignored. */
function extended(value: string): string | null {
  const match = /^utf-8'[^']*'(.*)$/i.exec(value)
  if (!match) return null
  const bytes: number[] = []
  const text = match[1] as string
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '%') {
      const hex = text.slice(i + 1, i + 3)
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return null
      bytes.push(parseInt(hex, 16))
      i += 2
    } else {
      bytes.push(...Buffer.from(text[i] as string))
    }
  }
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(bytes))
  } catch {
    return null
  }
}

/** The last segment of the path of an address, without query and fragment, percent-decoded where it can be. */
function lastSegment(path: string): string {
  const bare = path.split(/[?#]/)[0] as string
  const segment = bare.slice(bare.lastIndexOf('/') + 1)
  try {
    return decodeURIComponent(segment)
  } catch {
    return segment
  }
}

/** Characters no file name may hold on Windows (and that make a colon a drive or a stream). */
const RESERVED = new Set(['<', '>', ':', '"', '|', '?', '*'])
/** Names Windows gives to devices, with or without an extension. */
const DEVICE = /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/i
/** Longest name proposed, in UTF-8 bytes; an extension of up to 16 bytes is kept. */
export const MAX_NAME_BYTES = 200
const MAX_EXTENSION_BYTES = 16

const utf8 = (text: string) => Buffer.byteLength(text, 'utf8')

/** `text` cut to at most `bytes` UTF-8 bytes, never inside a character. */
function cutTo(text: string, bytes: number): string {
  let out = ''
  for (const c of text) {
    if (utf8(out) + utf8(c) > bytes) break
    out += c
  }
  return out
}

/** Spaces and dots off both ends: a leading dot would hide the file, Windows drops trailing ones. */
const stripEnds = (text: string) => text.replace(/^[ .]+/, '').replace(/[ .]+$/, '')

/** The name within MAX_NAME_BYTES, keeping a short extension. */
function capped(name: string): string {
  if (utf8(name) <= MAX_NAME_BYTES) return name
  const dot = name.lastIndexOf('.')
  const extension = dot > 0 && utf8(name.slice(dot)) <= MAX_EXTENSION_BYTES ? name.slice(dot) : ''
  const stem = stripEnds(cutTo(name.slice(0, name.length - extension.length), MAX_NAME_BYTES - utf8(extension)))
  return (stem === '' ? 'download' : stem) + extension
}

/**
 * A plain file name, the same on every system: what follows the last `/` or
 * `\`; control characters and < > : " | ? * become `_`; spaces and dots go
 * from both ends (no hidden file, nothing Windows would drop); nothing left
 * (".", "..") is "download"; at most MAX_NAME_BYTES; a Windows device name
 * gets a leading `_`. The cases are shared with the Rust host.
 */
export function plainName(raw: string): string {
  const cut = Math.max(raw.lastIndexOf('/'), raw.lastIndexOf('\\'))
  const mapped = [...raw.slice(cut + 1)].map((c) => (isControl(c.codePointAt(0) as number) || RESERVED.has(c) ? '_' : c)).join('')
  const stripped = stripEnds(mapped)
  if (stripped === '') return 'download'
  const name = capped(stripped)
  const stem = (name.split('.')[0] as string).replace(/ +$/, '')
  return DEVICE.test(stem) ? `_${name}` : name
}

/**
 * The name for a download: `filename*` of the disposition, else `filename`,
 * else the page's `download` attribute, else the last segment of `path`.
 * A candidate that is empty is skipped.
 */
export function downloadName(disposition: string | null, attribute: string, path: string): string {
  const params = disposition === null ? new Map<string, string>() : parameters(disposition)
  const star = params.get('filename*')
  const candidates = [star === undefined ? null : extended(star), params.get('filename') ?? null, attribute, lastSegment(path)]
  return plainName(candidates.find((c) => c !== null && c.trim() !== '') ?? '')
}

/**
 * `name` in `folder`, if it stays directly in it (the name is plain by
 * construction; this keeps a slip from ever writing elsewhere), else
 * "download" there.
 */
export function inFolder(folder: string, name: string): string {
  const path = resolve(folder, name)
  return dirname(path) === resolve(folder) && basename(path) === name ? path : join(resolve(folder), 'download')
}
