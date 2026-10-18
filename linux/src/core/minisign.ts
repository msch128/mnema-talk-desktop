/**
 * minisign verification of a downloaded package (the format of the Tauri
 * updater). The same rule as `verify_package` in
 * `experimental/app/src/native_updates.rs`: the package must verify against the
 * pinned public key, and the version recorded in the signed trusted comment
 * must equal the version the feed announced, so an old signed package cannot
 * be passed off under a newer version number.
 */
import { createHash } from 'node:crypto'
import { publicKeyFromRaw, verifyEd25519 } from './ed25519.ts'
import { strictBase64 } from './encoding.ts'
import { compareVersions, parseVersion } from './version.ts'

export type VerifyError = 'key' | 'signature' | 'mismatch' | 'version'

function text(wrapped: string): string | null {
  const raw = strictBase64(wrapped.trim())
  if (!raw) return null
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(raw)
  } catch {
    return null
  }
}

function lines(value: string): string[] {
  const out = value.split('\n').map((l) => l.replace(/\r$/, ''))
  if (out.at(-1) === '') out.pop()
  return out
}

interface MinisignKey { keyId: Buffer, raw: Buffer }
interface MinisignSignature { prehashed: boolean, keyId: Buffer, signature: Buffer, trustedComment: string, global: Buffer }

function decodeKey(value: string): MinisignKey | null {
  const bytes = strictBase64((lines(value)[1] ?? '').trim())
  if (!bytes || bytes.length !== 42 || bytes[0] !== 0x45 || bytes[1] !== 0x64) return null
  return { keyId: bytes.subarray(2, 10), raw: bytes.subarray(10) }
}

function decodeSignature(value: string): MinisignSignature | null {
  const [, sigLine, trustedLine, globalLine] = lines(value)
  const bytes = strictBase64((sigLine ?? '').trim())
  if (!bytes || bytes.length !== 74 || bytes[0] !== 0x45 || (bytes[1] !== 0x64 && bytes[1] !== 0x44)) return null
  if (!trustedLine?.startsWith('trusted comment: ')) return null
  const global = strictBase64((globalLine ?? '').trim())
  if (!global || global.length !== 64) return null
  return {
    prehashed: bytes[1] === 0x44,
    keyId: bytes.subarray(2, 10),
    signature: bytes.subarray(10),
    trustedComment: trustedLine.slice('trusted comment: '.length),
    global,
  }
}

export function verifyPackage(bytes: Uint8Array, signature: string, pubkey: string, version: string): VerifyError | null {
  const keyText = text(pubkey)
  const key = keyText === null ? null : decodeKey(keyText)
  const publicKey = key ? publicKeyFromRaw(key.raw) : null
  if (!key || !publicKey) return 'key'
  const sigText = text(signature)
  const sig = sigText === null ? null : decodeSignature(sigText)
  if (!sig) return 'signature'
  if (!key.keyId.equals(sig.keyId)) return 'mismatch'
  const message = sig.prehashed ? createHash('blake2b512').update(bytes).digest() : bytes
  if (!verifyEd25519(publicKey, message, sig.signature)) return 'mismatch'
  // Only after the package verified is the trusted comment covered by the signature.
  if (!verifyEd25519(publicKey, Buffer.concat([sig.signature, Buffer.from(sig.trustedComment)]), sig.global)) return 'mismatch'
  const signed = sig.trustedComment.split('\t').map((f) => (f.startsWith('version:') ? f.slice('version:'.length) : null)).find((f) => f !== null)
  const parsedSigned = signed === undefined || signed === null ? null : parseVersion(signed.replace(/^v+/, ''))
  const announced = parseVersion(version)
  if (!parsedSigned || !announced || compareVersions(parsedSigned, announced) !== 0) return 'version'
  return null
}
