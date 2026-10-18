import { createHash, createPublicKey, verify, type KeyObject } from 'node:crypto'

const SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex')
const P = (1n << 255n) - 19n
const D = 37095705934669439343138083508754565189542113879843219016388785533085940283555n

function pow(base: bigint, exponent: bigint): bigint {
  let result = 1n
  let b = base % P
  let e = exponent
  while (e > 0n) {
    if (e & 1n) result = (result * b) % P
    b = (b * b) % P
    e >>= 1n
  }
  return result
}

/**
 * Whether 32 bytes decompress to a point on the curve, which is what
 * `ed25519_dalek::VerifyingKey::from_bytes` checks. Node would take any 32
 * bytes and only fail later.
 */
export function isEd25519Point(raw: Uint8Array): boolean {
  if (raw.length !== 32) return false
  let y = 0n
  for (let i = 31; i >= 0; i--) y = (y << 8n) | BigInt(raw[i] as number)
  const sign = y >> 255n
  y &= (1n << 255n) - 1n
  if (y >= P) return false
  const y2 = (y * y) % P
  const numerator = (y2 - 1n + P) % P
  const denominator = (D * y2 + 1n) % P
  const x2 = (numerator * pow(denominator, P - 2n)) % P
  if (x2 === 0n) return sign === 0n
  return pow(x2, (P - 1n) / 2n) === 1n
}

export function publicKeyFromRaw(raw: Uint8Array): KeyObject | null {
  if (!isEd25519Point(raw)) return null
  try {
    return createPublicKey({ key: Buffer.concat([SPKI_PREFIX, raw]), format: 'der', type: 'spki' })
  } catch {
    return null
  }
}

export function verifyEd25519(key: KeyObject, message: Uint8Array, signature: Uint8Array): boolean {
  if (signature.length !== 64) return false
  try {
    return verify(null, message, key, signature)
  } catch {
    return false
  }
}

export function sha256Prefix(raw: Uint8Array, length: number): string {
  return createHash('sha256').update(raw).digest('hex').slice(0, length * 2)
}
