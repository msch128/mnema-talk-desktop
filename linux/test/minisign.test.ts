import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { verifyPackage } from '../src/core/minisign.ts'
import { minisignKey, minisignSignature } from './helpers/fixtures.ts'

const read = (name: string) => readFileSync(new URL(`./fixtures/${name}`, import.meta.url))

test('a signature written by the Tauri CLI verifies, bound to its version', () => {
  const sig = read('tauri-cli.sig').toString('utf8').trim()
  const pub = read('tauri-cli.pub').toString('utf8').trim()
  const pkg = read('tauri-cli.bin')
  assert.equal(verifyPackage(pkg, sig, pub, '1.2.3'), null)
  assert.equal(verifyPackage(pkg, sig, pub, '1.2.4'), 'version')
  assert.equal(verifyPackage(Buffer.from('other'), sig, pub, '1.2.3'), 'mismatch')
})

test('the package, the key, the trusted comment and the version all have to match', () => {
  const key = minisignKey(3)
  const other = minisignKey(4)
  const pkg = Buffer.from('package bytes')
  const good = minisignSignature(key, pkg, '1.0.0')
  assert.equal(verifyPackage(pkg, good, key.pubkey, '1.0.0'), null)
  assert.equal(verifyPackage(pkg, good, key.pubkey, 'v1.0.0'.slice(1)), null)
  assert.equal(verifyPackage(Buffer.from('package bytez'), good, key.pubkey, '1.0.0'), 'mismatch')
  assert.equal(verifyPackage(pkg, good, other.pubkey, '1.0.0'), 'mismatch')
  assert.equal(verifyPackage(pkg, good, key.pubkey, '1.0.1'), 'version')
  assert.equal(verifyPackage(pkg, good, key.pubkey, 'x'), 'version')
  assert.equal(verifyPackage(pkg, good, '%%%', '1.0.0'), 'key')
  assert.equal(verifyPackage(pkg, 'garbage', key.pubkey, '1.0.0'), 'signature')
  // A version prefixed with v in the comment counts, as the Rust host reads it.
  assert.equal(verifyPackage(pkg, minisignSignature(key, pkg, '1.0.0', { comment: 'timestamp:1\tversion:v1.0.0' }), key.pubkey, '1.0.0'), null)
  // No version in the signed comment.
  assert.equal(verifyPackage(pkg, minisignSignature(key, pkg, '1.0.0', { comment: 'timestamp:1' }), key.pubkey, '1.0.0'), 'version')
  // A comment edited after signing no longer verifies.
  const text = Buffer.from(good, 'base64').toString('utf8').replace('version:1.0.0', 'version:9.0.0')
  assert.equal(verifyPackage(pkg, Buffer.from(text).toString('base64'), key.pubkey, '9.0.0'), 'mismatch')
  // Legacy (not prehashed) signatures are accepted; the key id must match.
  assert.equal(verifyPackage(pkg, minisignSignature(key, pkg, '1.0.0', { prehashed: false }), key.pubkey, '1.0.0'), null)
  assert.equal(verifyPackage(pkg, minisignSignature(key, pkg, '1.0.0', { idBytes: Buffer.alloc(8, 9) }), key.pubkey, '1.0.0'), 'mismatch')
})
