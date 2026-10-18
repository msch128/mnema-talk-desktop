import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { LATEST_RELEASE_URL, PINNED_BUNDLE_KEYS, ROOT_PUB, TRUST_SIG_URL, TRUST_URL, UPDATER_ENDPOINTS, UPDATER_PUBKEY } from '../src/core/pinned.ts'

const app = (name: string) => readFileSync(new URL(`../../experimental/app/${name}`, import.meta.url), 'utf8')

test('the pinned values are the ones of the Tauri host', () => {
  const bundle = app('src/web_bundle.rs')
  const trust = app('src/web_trust.rs')
  const updates = app('src/web_updates.rs')
  const pinned = /const PINNED: &\[&str\] = &\[([^\]]*)\]/.exec(bundle)?.[1]?.match(/"([0-9a-f]+)"/g)?.map((s) => s.replaceAll('"', ''))
  assert.deepEqual([...PINNED_BUNDLE_KEYS], pinned)
  assert.equal(ROOT_PUB, /const ROOT: &str = "([0-9a-f]+)"/.exec(trust)?.[1])
  assert.equal(TRUST_URL, /pub const TRUST_URL: &str =\s*"([^"]+)"/.exec(trust)?.[1])
  assert.equal(TRUST_SIG_URL, /pub const TRUST_SIG_URL: &str =\s*"([^"]+)"/.exec(trust)?.[1])
  assert.equal(LATEST_RELEASE_URL, /const LATEST: &str = "([^"]+)"/.exec(updates)?.[1])
  const conf = JSON.parse(app('tauri.conf.json')) as { plugins: { updater: { pubkey: string, endpoints: string[], requireSignedVersion: boolean } } }
  assert.equal(UPDATER_PUBKEY, conf.plugins.updater.pubkey)
  assert.deepEqual([...UPDATER_ENDPOINTS], conf.plugins.updater.endpoints)
  assert.equal(conf.plugins.updater.requireSignedVersion, true)
})

test('the package version is the version of the Tauri app', () => {
  const conf = JSON.parse(app('tauri.conf.json')) as { version: string }
  const own = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as { version: string }
  assert.equal(own.version, conf.version)
})
