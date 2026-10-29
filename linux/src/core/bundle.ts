/**
 * Signed web app bundles (mnema-talk `doc/desktop-trusted-client.md`). A port of
 * `experimental/app/src/web_bundle.rs` (verification half); the shared vectors
 * in `test-vectors/` must pass in both.
 *
 * A bundle is one zip: `manifest.json`, `manifest.sig` (base64 Ed25519 over the
 * exact manifest bytes) and `app/<path>` for every file of the web app. Only
 * bundles whose signature matches a trusted key are run. Nothing here trusts
 * where bytes came from.
 */
import type { KeyObject } from 'node:crypto'
import { isEd25519Point, publicKeyFromRaw, sha256Prefix, verifyEd25519 } from './ed25519.ts'
import { asU32, asU64, parseStrictJson } from './json.ts'
import { isLowerHex, lossyTrimmed, sha256Hex, strictBase64 } from './encoding.ts'
import { type Version, compareVersions, parseVersion } from './version.ts'
import { ZipArchive, ZipError } from './zip.ts'

export const MAX_ENTRIES = 4000
export const MAX_FILE = 64 << 20
const MAX_TOTAL = 256 << 20
export const MAX_MANIFEST = 4 << 20
const MAX_SIGNATURE = 1024
export const MAX_PATH = 512
/** A cached bundle larger than this is not even read. */
export const MAX_ZIP = 96 << 20

/** A public key the client trusts. `id` selects the key; the signature decides. */
export interface TrustedKey {
  readonly id: string
  readonly raw: Buffer
  readonly key: KeyObject
}

export function keyId(raw: Uint8Array): string {
  return sha256Prefix(raw, 8)
}

export function parseKey(rawHex: string): TrustedKey | null {
  if (!/^(?:[0-9a-fA-F]{2})+$/.test(rawHex) || rawHex.length !== 64) return null
  const raw = Buffer.from(rawHex, 'hex')
  if (!isEd25519Point(raw)) return null
  const key = publicKeyFromRaw(raw)
  return key ? { id: keyId(raw), raw, key } : null
}

/** Bundle-relative path rules, shared with the signing tool. */
export function validPath(path: string): boolean {
  if (path.length === 0 || Buffer.byteLength(path) > MAX_PATH) return false
  for (const ch of path) {
    const code = ch.codePointAt(0) as number
    if (code < 0x20 || code === 0x7f || ch === '\\') return false
  }
  return path.split('/').every((segment) => segment !== '' && segment !== '.' && segment !== '..')
}

interface FileEntry { path: string, sha256: string, size: number }

interface Manifest {
  schema: number
  name: string
  version: string
  web_client_api: number
  min_server_version: string
  security_floor: string
  commit: string
  key_id: string
  files: FileEntry[]
}

const MANIFEST_FIELDS = ['schema', 'name', 'version', 'web_client_api', 'min_server_version', 'security_floor', 'commit', 'key_id', 'files']
const FILE_FIELDS = ['path', 'sha256', 'size']

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function exactFields(value: Record<string, unknown>, fields: string[]): boolean {
  const keys = Object.keys(value)
  return keys.length === fields.length && fields.every((f) => keys.includes(f))
}

/** serde's `deny_unknown_fields` and typed fields, by hand. */
function parseManifest(bytes: Uint8Array): Manifest | null {
  let raw: unknown
  try {
    raw = parseStrictJson(bytes)
  } catch {
    return null
  }
  if (!isRecord(raw) || !exactFields(raw, MANIFEST_FIELDS)) return null
  const { name, version, min_server_version, security_floor, commit, key_id, files } = raw
  const schema = asU32(raw.schema)
  const web_client_api = asU32(raw.web_client_api)
  if (schema === null || typeof name !== 'string' || typeof version !== 'string' || web_client_api === null) return null
  if (typeof min_server_version !== 'string' || typeof security_floor !== 'string' || typeof commit !== 'string' || typeof key_id !== 'string') return null
  if (!Array.isArray(files)) return null
  const parsed: FileEntry[] = []
  for (const file of files) {
    if (!isRecord(file) || !exactFields(file, FILE_FIELDS)) return null
    const size = asU64(file.size)
    if (typeof file.path !== 'string' || typeof file.sha256 !== 'string' || size === null) return null
    parsed.push({ path: file.path, sha256: file.sha256, size })
  }
  return { schema, name, version, web_client_api, min_server_version, security_floor, commit, key_id, files: parsed }
}

/** A verified bundle, held in memory. */
export class Bundle {
  readonly version: Version
  readonly webClientApi: number
  readonly minServerVersion: Version
  readonly securityFloor: Version
  readonly keyId: string
  readonly commit: string
  private readonly files: Map<string, Uint8Array>

  constructor(init: { version: Version, webClientApi: number, minServerVersion: Version, securityFloor: Version, keyId: string, commit: string, files: Map<string, Uint8Array> }) {
    this.version = init.version
    this.webClientApi = init.webClientApi
    this.minServerVersion = init.minServerVersion
    this.securityFloor = init.securityFloor
    this.keyId = init.keyId
    this.commit = init.commit
    this.files = init.files
  }

  file(path: string): Uint8Array | undefined {
    return this.files.get(path)
  }
}

export class BundleError extends Error {}

/** A release turned down for good (already held, below the floor, fails verification): looked at, not a failure to retry. */
export class BundleRefused extends BundleError {}

function fail(message: string): never {
  throw new BundleRefused(message)
}

function readEntry(zip: ZipArchive, index: number, limit: number): Uint8Array {
  try {
    return zip.read(zip.entries[index] as NonNullable<(typeof zip.entries)[number]>, limit)
  } catch (error) {
    if (error instanceof ZipError) fail(error.message)
    throw error
  }
}

/**
 * Verifies a bundle against the trusted keys: signature first (nothing in the
 * manifest is parsed before it holds), then that the zip holds exactly the
 * files the manifest lists with the listed sizes and hashes.
 */
export function verifyBundle(data: Uint8Array, keys: readonly TrustedKey[]): Bundle {
  let zip: ZipArchive
  try {
    zip = new ZipArchive(data)
  } catch (error) {
    if (error instanceof ZipError) fail(error.message)
    throw error
  }
  if (zip.length > MAX_ENTRIES + 2) fail('too many entries')
  const index = new Map<string, number>()
  for (let i = 0; i < zip.length; i++) {
    const entry = zip.entries[i] as (typeof zip.entries)[number]
    if (!entry.nameValid) fail('invalid entry name')
    if (index.has(entry.name)) fail(`duplicate entry ${entry.name}`)
    index.set(entry.name, i)
  }
  const manifestAt = index.get('manifest.json')
  const signatureAt = index.get('manifest.sig')
  if (manifestAt === undefined || signatureAt === undefined) fail('manifest or signature is missing')
  const manifestBytes = readEntry(zip, manifestAt, MAX_MANIFEST)
  const signatureText = readEntry(zip, signatureAt, MAX_SIGNATURE)
  const signature = strictBase64(lossyTrimmed(signatureText))
  if (!signature || signature.length !== 64) fail('malformed signature')
  const signer = keys.find((k) => verifyEd25519(k.key, manifestBytes, signature))
  if (!signer) fail('signature does not match a trusted key')

  const manifest = parseManifest(manifestBytes)
  if (!manifest) fail('unreadable manifest')
  const { version, min, floor } = checkManifest(manifest, signer.id)
  if (index.size !== manifest.files.length + 2) fail('zip entries do not match the manifest')
  const files = new Map<string, Uint8Array>()
  for (const file of manifest.files) {
    const at = index.get(`app/${file.path}`)
    if (at === undefined) fail(`${file.path}: missing from the zip`)
    if ((zip.entries[at] as (typeof zip.entries)[number]).size !== file.size) fail(`${file.path}: size does not match the manifest`)
    const body = readEntry(zip, at, file.size)
    if (sha256Hex(body) !== file.sha256) fail(`${file.path}: content does not match the manifest`)
    files.set(file.path, body)
  }
  return new Bundle({
    version,
    webClientApi: manifest.web_client_api,
    minServerVersion: min,
    securityFloor: floor,
    keyId: manifest.key_id,
    commit: manifest.commit,
    files,
  })
}

/** Checks the signed manifest; returns the versions it parsed so they are read once. */
function checkManifest(m: Manifest, signer: string): { version: Version, min: Version, floor: Version } {
  if (m.schema !== 1 || m.name !== 'mnema-web') fail('unsupported manifest')
  if (m.key_id !== signer) fail('manifest key_id does not match the signing key')
  const version = parseVersion(m.version) ?? fail('invalid version')
  const min = parseVersion(m.min_server_version) ?? fail('invalid min_server_version')
  const floor = parseVersion(m.security_floor) ?? fail('invalid security_floor')
  if (m.web_client_api < 1 || compareVersions(min, version) > 0 || compareVersions(floor, version) > 0) fail('invalid compatibility numbers')
  if (m.files.length === 0 || m.files.length > MAX_ENTRIES) fail('invalid file count')
  const seen = new Set<string>()
  let total = 0
  for (const file of m.files) {
    // Two paths that differ only in case would collide on a case-insensitive disk.
    const folded = file.path.toLowerCase()
    if (!validPath(file.path) || seen.has(folded)) fail(`invalid or duplicate path ${file.path}`)
    seen.add(folded)
    if (!isLowerHex(file.sha256, 64)) fail(`${file.path}: invalid sha256`)
    if (file.size > MAX_FILE) fail(`${file.path}: invalid size`)
    total += file.size
  }
  if (!m.files.some((f) => f.path === 'index.html')) fail('index.html is missing')
  if (total > MAX_TOTAL) fail('bundle exceeds the size limit')
  return { version, min, floor }
}
