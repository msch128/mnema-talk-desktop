import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { verifyPackage } from '../../src/core/minisign.ts'
import { minisignKey, minisignSignature } from '../../test/helpers/fixtures.ts'
import { root, type Json } from './common.ts'

export function minisignVectors(): Json {
  const key = minisignKey(3)
  const other = minisignKey(4)
  const pkg = 'package bytes'
  const content = Buffer.from(pkg)
  const sign = (m = key, bytes: Uint8Array = content, version = '1.0.0', options: Parameters<typeof minisignSignature>[3] = {}) => minisignSignature(m, bytes, version, options)
  const good = sign()
  const tamperedComment = Buffer.from(Buffer.from(good, 'base64').toString('utf8').replace('version:1.0.0', 'version:9.0.0')).toString('base64')
  const cases: { name: string, package: string, signature: string, pubkey: string, version: string }[] = [
    { name: 'a prehashed signature', package: pkg, signature: good, pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a legacy signature', package: pkg, signature: sign(key, content, '1.0.0', { prehashed: false }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a version with a v in the signed comment', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1\tversion:v1.0.0' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a version among other fields', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1\tfile:x\tversion:1.0.0\textra:y' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'another package', package: 'package bytez', signature: good, pubkey: key.pubkey, version: '1.0.0' },
    { name: 'another key', package: pkg, signature: good, pubkey: other.pubkey, version: '1.0.0' },
    { name: 'another announced version', package: pkg, signature: good, pubkey: key.pubkey, version: '1.0.1' },
    { name: 'an announced version that is not a version', package: pkg, signature: good, pubkey: key.pubkey, version: 'x' },
    { name: 'a key that is not a key', package: pkg, signature: good, pubkey: '%%%', version: '1.0.0' },
    { name: 'a signature that is not a signature', package: pkg, signature: 'garbage', pubkey: key.pubkey, version: '1.0.0' },
    { name: 'no version in the signed comment', package: pkg, signature: sign(key, content, '1.0.0', { comment: 'timestamp:1' }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a trusted comment edited after signing', package: pkg, signature: tamperedComment, pubkey: key.pubkey, version: '9.0.0' },
    { name: 'a signature naming another key id', package: pkg, signature: sign(key, content, '1.0.0', { idBytes: Buffer.alloc(8, 9) }), pubkey: key.pubkey, version: '1.0.0' },
    { name: 'a signature of another key under the right id', package: pkg, signature: sign(other, content, '1.0.0', { idBytes: key.idBytes }), pubkey: key.pubkey, version: '1.0.0' },
  ]
  const real = {
    name: 'a signature written by the Tauri CLI',
    package: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.bin'), 'utf8'),
    signature: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.sig'), 'utf8').trim(),
    pubkey: readFileSync(join(root, '..', 'linux', 'test', 'fixtures', 'tauri-cli.pub'), 'utf8').trim(),
    version: '1.2.3',
  }
  const all = [...cases, real, { ...real, name: 'a signature written by the Tauri CLI, another version', version: '1.2.4' }]
  return {
    cases: all.map((c) => ({ ...c, expect: verifyPackage(Buffer.from(c.package), c.signature, c.pubkey, c.version) })),
  }
}
