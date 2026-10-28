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

/** The group order of the prime-order subgroup. */
const L = (1n << 252n) + 27742317777372353535851937790883648493n

/** The eight points of small order (the torsion subgroup), compressed. */
const SMALL_ORDER = new Set([
  '0100000000000000000000000000000000000000000000000000000000000000',
  'ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f',
  '0000000000000000000000000000000000000000000000000000000000000000',
  '0000000000000000000000000000000000000000000000000000000000000080',
  '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05',
  '26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85',
  'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a',
  'c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa',
])

/** Whether the compressed point has small order: a signature under such a key or with such an R proves nothing. */
function isSmallOrder(raw: Uint8Array): boolean {
  return SMALL_ORDER.has(Buffer.from(raw).toString('hex'))
}

/**
 * Keys as `ed25519_dalek::VerifyingKey::from_bytes` and `verify_strict` take
 * them: on the curve and not of small order.
 */
export function publicKeyFromRaw(raw: Uint8Array): KeyObject | null {
  if (!isEd25519Point(raw) || isSmallOrder(raw)) return null
  try {
    return createPublicKey({ key: Buffer.concat([SPKI_PREFIX, raw]), format: 'der', type: 'spki' })
  } catch {
    return null
  }
}

/**
 * `verify_strict`: on top of the plain check, R must not be of small order and
 * S must be canonical (below the group order). A non-canonical R cannot match
 * the recomputed one, which the plain check already compares.
 */
export function verifyEd25519(key: KeyObject, message: Uint8Array, signature: Uint8Array): boolean {
  if (signature.length !== 64) return false
  if (isSmallOrder(signature.subarray(0, 32))) return false
  let s = 0n
  for (let i = 63; i >= 32; i--) s = (s << 8n) | BigInt(signature[i] as number)
  if (s >= L) return false
  try {
    return verify(null, message, key, signature)
  } catch {
    return false
  }
}

export function sha256Prefix(raw: Uint8Array, length: number): string {
  return createHash('sha256').update(raw).digest('hex').slice(0, length * 2)
}
