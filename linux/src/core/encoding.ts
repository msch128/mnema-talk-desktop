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

/** `String::from_utf8_lossy(bytes).trim()` */
export function lossyTrimmed(bytes: Uint8Array): string {
  return new TextDecoder('utf-8', { fatal: false }).decode(bytes).trim()
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex')
}

export function isLowerHex(text: string, length: number): boolean {
  return text.length === length && /^[0-9a-f]+$/.test(text)
}
