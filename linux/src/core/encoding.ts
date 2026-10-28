import { createHash } from 'node:crypto'

const STRICT_BASE64 = /^[A-Za-z0-9+/]*={0,2}$/

/**
 * Standard base64 with padding, canonical encodings only (what Rust's
 * `base64::STANDARD` accepts). Node's own decoder skips junk; a signature or
 * key must not.
 */
export function strictBase64(text: string): Buffer | null {
  if (text.length % 4 !== 0 || !STRICT_BASE64.test(text)) return null
  const raw = Buffer.from(text, 'base64')
  return raw.toString('base64') === text ? raw : null
}

/** Rust's `str::trim` removes the Unicode White_Space set; JavaScript's `trim` differs (it strips U+FEFF and keeps U+0085). */
const WHITE_SPACE = '\\t-\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000'
const RUST_TRIM = new RegExp(`^[${WHITE_SPACE}]+|[${WHITE_SPACE}]+$`, 'gu')

/** `String::from_utf8_lossy(bytes).trim()` */
export function lossyTrimmed(bytes: Uint8Array): string {
  // `ignoreBOM` keeps a leading byte order mark, as `from_utf8_lossy` does.
  return new TextDecoder('utf-8', { fatal: false, ignoreBOM: true }).decode(bytes).replace(RUST_TRIM, '')
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex')
}

export function isLowerHex(text: string, length: number): boolean {
  return text.length === length && /^[0-9a-f]+$/.test(text)
}

/**
 * Strict UTF-8 as Rust's `String::from_utf8` reads it: invalid bytes give null
 * and a leading byte order mark stays in the text (a decoder's default is to
 * drop it silently, so a BOM-prefixed name or document would pass here and be
 * refused by the Tauri host).
 */
export function strictUtf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes)
  } catch {
    return null
  }
}

/** `serde_json::from_slice`: strict UTF-8 without a byte order mark, then JSON. Throws on anything else. */
export function strictJson(bytes: Uint8Array): unknown {
  const text = strictUtf8(bytes)
  if (text === null) throw new Error('not UTF-8')
  return JSON.parse(text)
}
