import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { verifyBundle } from '../../src/core/bundle.ts'
import { formatVersion } from '../../src/core/version.ts'
import { sha256, testKey, trusted } from '../../test/helpers/fixtures.ts'

export const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'test-vectors')
export const b64 = (bytes: Uint8Array) => Buffer.from(bytes).toString('base64')
export const hex = (seed: number) => testKey(seed).hex
export const keysOf = (...seeds: number[]) => seeds.map(hex)

export type Json = Record<string, unknown>

/** What `verifyBundle` answers for one zip: the fields of a good bundle or the refusal. */
export function outcome(name: string, keys: string[], zip: Buffer): { name: string, trusted: string[], zip: string, result: Json } {
  let result: Json
  try {
    const b = verifyBundle(zip, trusted(...keys.map((k) => [7, 5, 8].find((s) => hex(s) === k) as number)))
    const paths = ['index.html', 'assets/app-1.js', '\u00fcn\u00ef/x.js'].filter((p) => b.file(p) !== undefined)
    result = {
      ok: {
        version: formatVersion(b.version), web_client_api: b.webClientApi, min_server_version: formatVersion(b.minServerVersion),
        security_floor: formatVersion(b.securityFloor), key_id: b.keyId, commit: b.commit,
        files: Object.fromEntries(paths.map((p) => [p, sha256(b.file(p) as Uint8Array)])),
      },
    }
  } catch (error) {
    result = { error: (error as Error).message }
  }
  return { name, trusted: keys, zip: b64(zip), result }
}
