/**
 * Builders for throwaway signed bundles, trust statements and minisign
 * signatures. Keys come from fixed one-byte seeds, so the output is
 * deterministic and `scripts/gen-vectors.ts` can commit it as shared vectors.
 * Nothing here is a real key.
 */
import { createHash, createPrivateKey, createPublicKey, sign as edSign, type KeyObject } from 'node:crypto'
import { crc32, deflateRawSync } from 'node:zlib'
import { type TrustedKey, keyId, parseKey } from '../../src/core/bundle.ts'

const PKCS8_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex')

export interface TestKey {
  seed: number
  privateKey: KeyObject
  raw: Buffer
  hex: string
  id: string
}

export function testKey(seed: number): TestKey {
  const privateKey = createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, Buffer.alloc(32, seed)]), format: 'der', type: 'pkcs8' })
  const der = createPublicKey(privateKey).export({ format: 'der', type: 'spki' })
  const raw = der.subarray(der.length - 32)
  return { seed, privateKey, raw, hex: raw.toString('hex'), id: keyId(raw) }
}

export function trusted(...seeds: number[]): TrustedKey[] {
  return seeds.map((seed) => parseKey(testKey(seed).hex) as TrustedKey)
}

export function signWith(key: TestKey, message: Uint8Array): Buffer {
  return edSign(null, message, key.privateKey)
}

// ------------------------------------------------------------------- zip

export interface ZipItem {
  name: string
  body: Uint8Array
  /** 0 = stored (default), 8 = deflated. */
  method?: 0 | 8
  /** Overrides, to build damaged archives. */
  crc?: number
  declaredSize?: number
  flags?: number
}

export function zipOf(items: ZipItem[]): Buffer {
  const parts: Buffer[] = []
  const central: Buffer[] = []
  let offset = 0
  for (const item of items) {
    const name = Buffer.from(item.name)
    const body = Buffer.from(item.body)
    const method = item.method ?? 0
    const packed = method === 8 ? deflateRawSync(body) : body
    const crc = item.crc ?? crc32(body)
    const flags = (item.flags ?? 0) | 0x800
    const local = Buffer.alloc(30)
    local.writeUInt32LE(0x04034b50, 0)
    local.writeUInt16LE(20, 4)
    local.writeUInt16LE(flags, 6)
    local.writeUInt16LE(method, 8)
    local.writeUInt32LE(crc, 14)
    local.writeUInt32LE(packed.length, 18)
    local.writeUInt32LE(body.length, 22)
    local.writeUInt16LE(name.length, 26)
    parts.push(local, name, packed)
    const entry = Buffer.alloc(46)
    entry.writeUInt32LE(0x02014b50, 0)
    entry.writeUInt16LE(20, 4)
    entry.writeUInt16LE(20, 6)
    entry.writeUInt16LE(flags, 8)
    entry.writeUInt16LE(method, 10)
    entry.writeUInt32LE(crc, 16)
    entry.writeUInt32LE(packed.length, 20)
    entry.writeUInt32LE(item.declaredSize ?? body.length, 24)
    entry.writeUInt16LE(name.length, 28)
    entry.writeUInt32LE(offset, 42)
    central.push(entry, name)
    offset += local.length + name.length + packed.length
  }
  const directory = Buffer.concat(central)
  const end = Buffer.alloc(22)
  end.writeUInt32LE(0x06054b50, 0)
  end.writeUInt16LE(Math.min(items.length, 0xfffe), 8)
  end.writeUInt16LE(Math.min(items.length, 0xfffe), 10)
  end.writeUInt32LE(directory.length, 12)
  end.writeUInt32LE(offset, 16)
  return Buffer.concat([...parts, directory, end])
}

// ---------------------------------------------------------------- bundles

export interface Spec {
  version: string
  api: number
  minServer: string
  floor: string
  seed: number
  files: [string, string][]
}

export function spec(version: string, change: Partial<Spec> = {}): Spec {
  return {
    version,
    api: 1,
    minServer: '0.1.0',
    floor: '0.0.0',
    seed: 7,
    files: [['index.html', '<html>app</html>'], ['assets/app-1.js', 'console.log(1)']],
    ...change,
  }
}

export function sha256(text: string | Uint8Array): string {
  return createHash('sha256').update(text).digest('hex')
}

export function manifestOf(s: Spec): Record<string, unknown> {
  return {
    schema: 1,
    name: 'mnema-web',
    version: s.version,
    web_client_api: s.api,
    min_server_version: s.minServer,
    security_floor: s.floor,
    commit: 'test',
    key_id: testKey(s.seed).id,
    files: s.files.map(([path, body]) => ({ path, sha256: sha256(body), size: Buffer.byteLength(body) })),
  }
}

export interface Entry { name: string, body: Buffer }

/** Entries (name, bytes) of a bundle signed by the key of `s.seed`. */
export function entries(s: Spec): Entry[] {
  const manifest = Buffer.from(JSON.stringify(manifestOf(s)))
  const out: Entry[] = [
    { name: 'manifest.json', body: manifest },
    { name: 'manifest.sig', body: Buffer.from(signWith(testKey(s.seed), manifest).toString('base64')) },
  ]
  for (const [path, body] of s.files) out.push({ name: `app/${path}`, body: Buffer.from(body) })
  return out
}

export function zipEntries(list: Entry[]): Buffer {
  return zipOf(list.map((e) => ({ name: e.name, body: e.body })))
}

export function bundle(s: Spec): Buffer {
  return zipEntries(entries(s))
}

export function entry(list: Entry[], name: string): Entry {
  const found = list.find((e) => e.name === name)
  if (!found) throw new Error(`no entry ${name}`)
  return found
}

/** A bundle with an edited manifest, re-signed so that only the manifest rules are exercised. */
export function resigned(edit: (manifest: Record<string, unknown>) => void, s: Spec = spec('0.8.0')): Buffer {
  const list = entries(s)
  const manifest = JSON.parse(entry(list, 'manifest.json').body.toString('utf8')) as Record<string, unknown>
  edit(manifest)
  const raw = Buffer.from(JSON.stringify(manifest))
  entry(list, 'manifest.json').body = raw
  entry(list, 'manifest.sig').body = Buffer.from(signWith(testKey(s.seed), raw).toString('base64'))
  return zipEntries(list)
}

// ------------------------------------------------------- trust statements

export interface StatementSpec {
  schema?: number
  serial?: number
  issued_at?: string
  expires_at?: string
  bundle_keys?: string[]
  revoked_bundle_keys?: string[]
  updater_keys?: string[]
  revoked_updater_keys?: string[]
  max_version?: Record<string, string>
}

export function statementText(s: StatementSpec): string {
  const body: Record<string, unknown> = {
    schema: s.schema ?? 1,
    serial: s.serial ?? 1,
    issued_at: s.issued_at ?? '2026-01-01T00:00:00Z',
    expires_at: s.expires_at ?? '2026-06-30T00:00:00Z',
    bundle_keys: s.bundle_keys ?? [testKey(7).hex],
    revoked_bundle_keys: s.revoked_bundle_keys ?? [],
    updater_keys: s.updater_keys ?? [],
    revoked_updater_keys: s.revoked_updater_keys ?? [],
  }
  if (s.max_version) body.max_version = s.max_version
  return JSON.stringify(body)
}

export function signText(key: TestKey, text: string): string {
  return signWith(key, Buffer.from(text)).toString('base64')
}

// --------------------------------------------------------------- minisign

export interface MinisignKey {
  seed: number
  key: TestKey
  /** Key id as it appears in the file (8 bytes). */
  idBytes: Buffer
  /** The `.pub` file text, base64 encoded (the form of `tauri.conf.json`). */
  pubkey: string
  idHex: string
}

export function minisignKey(seed: number): MinisignKey {
  const key = testKey(seed)
  const idBytes = Buffer.alloc(8, seed)
  const line = Buffer.concat([Buffer.from('Ed'), idBytes, key.raw]).toString('base64')
  const text = `untrusted comment: minisign public key: ${idBytes.toString('hex').toUpperCase()}\n${line}\n`
  return { seed, key, idBytes, pubkey: Buffer.from(text).toString('base64'), idHex: idBytes.toString('hex') }
}

export interface SignOptions {
  /** Signed trusted comment; default `timestamp:1\tfile:x\tversion:<v>`. */
  comment?: string
  /** `ED` (prehashed, what the Tauri CLI writes) or legacy `Ed`. */
  prehashed?: boolean
  /** The key id to write into the signature. */
  idBytes?: Buffer
}

/** The base64 of a minisign `.sig` file, as the feed carries it. */
export function minisignSignature(signer: MinisignKey, content: Uint8Array, version: string, options: SignOptions = {}): string {
  const prehashed = options.prehashed ?? true
  const comment = options.comment ?? `timestamp:1\tfile:x\tversion:${version}`
  const message = prehashed ? createHash('blake2b512').update(content).digest() : Buffer.from(content)
  const signature = signWith(signer.key, message)
  const line = Buffer.concat([Buffer.from(prehashed ? 'ED' : 'Ed'), options.idBytes ?? signer.idBytes, signature]).toString('base64')
  const global = signWith(signer.key, Buffer.concat([signature, Buffer.from(comment)])).toString('base64')
  const text = `untrusted comment: signature from minisign secret key\n${line}\ntrusted comment: ${comment}\n${global}\n`
  return Buffer.from(text).toString('base64')
}
