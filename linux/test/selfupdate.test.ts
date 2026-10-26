import assert from 'node:assert/strict'
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { CHECK_INTERVAL_SECS } from '../src/core/store.ts'
import { SelfUpdater, type Snapshot, type UpdaterEnv, checkDue, downloadUrlOk, isSkipped, menuText, offerable, parseFeed, platformKey, replaceAppImage, trimNotes } from '../src/core/selfupdate.ts'
import type { Statement } from '../src/core/trust.ts'
import { minisignKey, minisignSignature } from './helpers/fixtures.ts'
import { serve, type TestServer } from './helpers/server.ts'

const servers: TestServer[] = []
const dirs: string[] = []
afterEach(async () => {
  for (const s of servers.splice(0)) await s.close()
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

const signer = minisignKey(3)
const PLATFORM = 'linux-x86_64'

interface Release { version: string, package: Buffer, signature?: string, notes?: string, platform?: string }

async function feed(release: () => Release | null): Promise<TestServer> {
  const server = await serve((r, res) => {
    const current = release()
    if (r.url === '/latest.json') {
      if (!current) { res.writeHead(404); res.end(); return }
      const host = `http://127.0.0.1:${r.headers.host?.split(':')[1]}`
      const body = JSON.stringify({
        version: current.version,
        notes: current.notes ?? 'Fixes.',
        pub_date: '2026-01-01T00:00:00Z',
        platforms: { [current.platform ?? PLATFORM]: { signature: current.signature ?? minisignSignature(signer, current.package, current.version), url: `${host}/pkg` } },
      })
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': String(Buffer.byteLength(body)) })
      res.end(body)
      return
    }
    res.writeHead(200, { 'content-length': String(current?.package.length ?? 0) })
    res.end(current?.package)
  })
  servers.push(server)
  return server
}

interface Harness {
  updater: SelfUpdater
  env: UpdaterEnv
  installed: { bytes: Buffer, version: string }[]
  restarts: number
  announced: Snapshot[]
  changes: Snapshot[]
}

function harness(server: TestServer, dir: string | null, change: Partial<UpdaterEnv> = {}): Harness {
  const h = { installed: [] as Harness['installed'], restarts: 0, announced: [] as Snapshot[], changes: [] as Snapshot[] } as Harness
  const env: UpdaterEnv = {
    current: '0.1.0',
    dir,
    endpoints: [new URL('/latest.json', server.origin)],
    pubkey: signer.pubkey,
    platform: PLATFORM,
    statement: () => null,
    installsAllowed: () => true,
    install: async (bytes, version) => { h.installed.push({ bytes, version }) },
    restart: () => { h.restarts += 1 },
    now: () => 1_000_000,
    githubOnly: false,
    fetchOptions: { allowHttp: true },
    external: false,
    onChange: (s) => h.changes.push(s),
    announce: (s) => h.announced.push(s),
    ...change,
  }
  h.env = env
  h.updater = new SelfUpdater(env)
  return h
}

function dir(): string {
  const d = mkdtempSync(join(tmpdir(), 'mnema-self-'))
  dirs.push(d)
  return d
}

const PKG = Buffer.concat([Buffer.from([0x7f, 0x45, 0x4c, 0x46]), Buffer.from('appimage payload')])

test('pure rules: versions, skips, intervals, urls, platforms and notes', () => {
  assert.deepEqual(offerable('0.1.0', '0.1.1'), [0, 1, 1])
  assert.deepEqual(offerable('0.3.9', '1.2.0'), [1, 2, 0])
  assert.equal(offerable('0.1.1', '0.1.1'), null)
  assert.equal(offerable('0.2.0', '0.1.9'), null)
  assert.equal(offerable('0.1.0', 'v0.1.1'), null)
  assert.equal(offerable('0.1.0', '0.1.1-rc1'), null)
  assert.ok(isSkipped('0.2.0', '0.2.0'))
  assert.ok(!isSkipped('0.2.0', '0.2.1'))
  assert.ok(!isSkipped(null, '0.2.1'))
  assert.ok(!checkDue(1000, 1000 + CHECK_INTERVAL_SECS - 1))
  assert.ok(checkDue(1000, 1000 + CHECK_INTERVAL_SECS))
  assert.ok(checkDue(5000, 1000), 'a clock set back counts as due')
  assert.ok(downloadUrlOk(new URL('https://github.com/msch128/mnema-talk-desktop/releases/download/v1.0.0/a.AppImage')))
  for (const bad of ['http://github.com/a', 'https://evil.example/a', 'https://github.com.evil.example/a', 'https://user@github.com/a', 'https://github.com:8443/a', 'https://api.github.com/a']) {
    assert.ok(!downloadUrlOk(new URL(bad)), bad)
  }
  assert.equal(platformKey('linux', 'x64'), 'linux-x86_64')
  assert.equal(platformKey('linux', 'arm64'), 'linux-aarch64')
  assert.equal(platformKey('darwin', 'arm64'), null)
  assert.equal(platformKey('linux', 'ia32'), null)
  assert.equal(platformKey('linux', 'x64', true), 'linux-x86_64-deb')
  assert.equal(platformKey('linux', 'arm64', true), 'linux-aarch64-deb')
  assert.equal(platformKey('darwin', 'arm64', true), null)
  assert.equal(trimNotes('a\u0000b\nc\u0007'), 'ab\nc')
  assert.equal(trimNotes('   '), null)
  assert.equal(trimNotes('x'.repeat(5000))?.length, 2000)
  assert.equal(menuText('available'), 'Update •')
  assert.equal(menuText('idle'), 'Updates')
})

test('the feed of the Tauri clients carries one entry per platform; this client reads its own', () => {
  const text = JSON.stringify({
    version: '0.2.0', notes: 'n', pub_date: 'x',
    platforms: {
      'windows-x86_64': { signature: 'w', url: 'https://github.com/a/b/w.exe' },
      'darwin-aarch64': { signature: 'm', url: 'https://github.com/a/b/m.tar.gz' },
      'linux-x86_64': { signature: 'l', url: 'https://github.com/a/b/l.AppImage' },
    },
  })
  assert.deepEqual(parseFeed(Buffer.from(text), 'linux-x86_64'), { version: '0.2.0', notes: 'n', signature: 'l', url: new URL('https://github.com/a/b/l.AppImage') })
  assert.equal(parseFeed(Buffer.from(text), 'linux-aarch64'), null)
  assert.equal(parseFeed(Buffer.from('nope'), 'linux-x86_64'), null)
})

test('a newer version is found, announced once and downloaded, verified and installed', async () => {
  const server = await feed(() => ({ version: '0.2.0', package: PKG }))
  const h = harness(server, dir())
  await h.updater.check(false)
  assert.equal(h.updater.snapshot().phase, 'available')
  assert.equal(h.updater.snapshot().version, '0.2.0')
  assert.equal(h.updater.snapshot().notes, 'Fixes.')
  assert.equal(h.announced.length, 1)
  await h.updater.install('now')
  assert.equal(h.installed.length, 1)
  assert.equal(h.installed[0]?.version, '0.2.0')
  assert.deepEqual(h.installed[0]?.bytes, PKG)
  assert.equal(h.restarts, 1)
  assert.equal(h.updater.snapshot().phase, 'installing')
  assert.ok(h.changes.some((s) => s.phase === 'downloading' && s.mode === 'now'))
  assert.equal(h.changes.filter((s) => s.phase === 'downloading').at(-1)?.received, PKG.length)
})

test('nothing newer, no entry for this platform and an unreachable feed', async () => {
  const same = await feed(() => ({ version: '0.1.0', package: PKG }))
  const h = harness(same, null)
  await h.updater.check(true)
  assert.equal(h.updater.snapshot().phase, 'up_to_date')
  assert.equal(h.announced.length, 0)
  await h.updater.check(false)
  assert.equal(h.updater.snapshot().phase, 'idle')
  // A release made before Linux builds existed has no entry: up to date, silently.
  const windowsOnly = await feed(() => ({ version: '0.9.0', package: PKG, platform: 'windows-x86_64' }))
  const w = harness(windowsOnly, null)
  await w.updater.check(true)
  assert.equal(w.updater.snapshot().phase, 'up_to_date')
  const gone = await feed(() => null)
  const g = harness(gone, null)
  await g.updater.check(false)
  assert.equal(g.updater.snapshot().phase, 'idle', 'a background check stays silent')
  await g.updater.check(true)
  assert.deepEqual([g.updater.snapshot().phase, g.updater.snapshot().error], ['failed', 'offline'])
})

test('a package whose signature does not verify is never installed', async () => {
  const other = minisignKey(4)
  for (const release of [
    { version: '0.2.0', package: PKG, signature: minisignSignature(other, PKG, '0.2.0') }, // another key
    { version: '0.2.0', package: PKG, signature: minisignSignature(signer, Buffer.from('different'), '0.2.0') }, // another package
    { version: '0.2.0', package: PKG, signature: minisignSignature(signer, PKG, '0.1.5') }, // an old signed version under a new number
    { version: '0.2.0', package: PKG, signature: 'bm9wZQ==' },
  ] satisfies Release[]) {
    const server = await feed(() => release)
    const h = harness(server, null)
    await h.updater.check(false)
    await h.updater.install('now')
    assert.deepEqual([h.updater.snapshot().phase, h.updater.snapshot().error], ['failed', 'verify'], release.signature?.slice(0, 12))
    assert.equal(h.installed.length, 0)
    assert.equal(h.restarts, 0)
  }
})

test('a revoked updater key, a missing fresh statement and a key the statement does not list refuse the update', async () => {
  const server = await feed(() => ({ version: '0.2.0', package: PKG }))
  const statement = (revoked: string[], allowed: string[]): Statement => ({
    schema: 1, serial: 2, issued_at: 'x', expires_at: 'y', bundle_keys: [], revoked_bundle_keys: [], updater_keys: allowed, revoked_updater_keys: revoked, max_version: {},
  })
  const revoked = harness(server, null, { statement: () => statement([signer.idHex], []) })
  await revoked.updater.check(true)
  assert.deepEqual([revoked.updater.snapshot().phase, revoked.updater.snapshot().error], ['failed', 'verify'])
  const unlisted = harness(server, null, { statement: () => statement([], [minisignKey(4).pubkey]) })
  await unlisted.updater.check(true)
  assert.equal(unlisted.updater.snapshot().error, 'verify')
  // A replacement key listed by the statement is used.
  const rotatedServer = await feed(() => ({ version: '0.2.0', package: PKG, signature: minisignSignature(minisignKey(4), PKG, '0.2.0') }))
  const rotated = harness(rotatedServer, null, { statement: () => statement([signer.idHex], [minisignKey(4).pubkey]) })
  await rotated.updater.check(false)
  await rotated.updater.install('now')
  assert.equal(rotated.installed.length, 1)
  // No fresh statement: nothing new is installed.
  const stale = harness(server, null, { installsAllowed: () => false })
  await stale.updater.check(false)
  await stale.updater.install('now')
  assert.deepEqual([stale.updater.snapshot().phase, stale.updater.snapshot().error], ['failed', 'verify'])
  assert.equal(stale.installed.length, 0)
})

test('on next start keeps the verified package and installs it after the feed agrees', async () => {
  const d = dir()
  let release: Release | null = { version: '0.2.0', package: PKG }
  const server = await feed(() => release)
  const first = harness(server, d)
  await first.updater.check(false)
  await first.updater.install('later')
  assert.equal(first.updater.snapshot().phase, 'ready')
  assert.equal(first.installed.length, 0)
  assert.ok(existsSync(join(d, 'native-update.pending')))
  assert.equal(statSync(join(d, 'native-update.pending')).mode & 0o777, 0o600)

  // The next start: the held package is announced as ready, and the feed still names it.
  const second = harness(server, d)
  assert.deepEqual([second.updater.snapshot().phase, second.updater.snapshot().version], ['ready', '0.2.0'])
  assert.equal(await second.updater.applyPending(), true)
  assert.equal(second.installed.length, 1)
  assert.deepEqual(second.installed[0]?.bytes, PKG)
  assert.equal(second.restarts, 1)

  // Installing "now" after a restart reuses the verified file instead of downloading again.
  const third = harness(server, d)
  await third.updater.check(false)
  const requests = server.requests.length
  await third.updater.install('now')
  assert.equal(server.requests.filter((r) => r.url === '/pkg').length, 1, 'downloaded once')
  assert.ok(server.requests.length > requests - 1)
  assert.equal(third.installed.length, 1)

  // A newer release replaces the held package: it is stale and discarded.
  release = { version: '0.3.0', package: Buffer.concat([PKG, Buffer.from('v3')]) }
  const fourth = harness(server, d)
  assert.equal(await fourth.updater.applyPending(), false)
  assert.equal(fourth.installed.length, 0)
  assert.equal(existsSync(join(d, 'native-update.pending')), false)
})

test('a kept package is not run when it changed on disk, the feed is offline, or no fresh statement is in force', async () => {
  const d = dir()
  const release: Release = { version: '0.2.0', package: PKG }
  const server = await feed(() => release)
  const first = harness(server, d)
  await first.updater.check(false)
  await first.updater.install('later')

  // No fresh statement: kept for later, nothing runs.
  const blocked = harness(server, d, { installsAllowed: () => false })
  assert.equal(await blocked.updater.applyPending(), false)
  assert.equal(blocked.installed.length, 0)
  assert.ok(existsSync(join(d, 'native-update.pending')))
  // The statement round is tried first when none is in force.
  let rounds = 0
  let allowed = false
  const rounded = harness(server, d, { installsAllowed: () => allowed, trustRound: async () => { rounds += 1; allowed = true } })
  assert.equal(await rounded.updater.applyPending(), true)
  assert.equal(rounds, 1)

  // Offline: kept for the next start.
  const again = dir()
  const second = harness(server, again)
  await second.updater.check(false)
  await second.updater.install('later')
  const offline = harness({ ...server, origin: new URL('http://127.0.0.1:1') }, again)
  assert.equal(await offline.updater.applyPending(), false)
  assert.ok(existsSync(join(again, 'native-update.pending')))

  // Changed on disk: discarded.
  writeFileSync(join(again, 'native-update.pending'), Buffer.concat([PKG, Buffer.from('x')]))
  const tampered = harness(server, again)
  assert.equal(await tampered.updater.applyPending(), false)
  assert.equal(tampered.installed.length, 0)
  assert.equal(existsSync(join(again, 'native-update.pending')), false)
})

test('a skipped version stays quiet and a newer one is offered again; a version already installed clears the held package', async () => {
  const d = dir()
  let version = '0.2.0'
  const server = await feed(() => ({ version, package: PKG }))
  const h = harness(server, d)
  await h.updater.check(false)
  h.updater.skip()
  assert.equal(h.updater.snapshot().phase, 'idle')
  await h.updater.check(false)
  assert.equal(h.announced.length, 1, 'a skipped version is not announced again')
  assert.equal(h.updater.snapshot().phase, 'available', 'but a manual look still shows it')
  version = '0.2.1'
  await h.updater.check(false)
  assert.equal(h.announced.length, 2)

  const held = dir()
  const keep = harness(server, held)
  await keep.updater.check(false)
  await keep.updater.install('later')
  const upgraded = harness(server, held, { current: '0.2.1' })
  assert.equal(upgraded.updater.hasPending(), false)
  assert.equal(upgraded.updater.snapshot().phase, 'idle')
  assert.equal(existsSync(join(held, 'native-update.pending')), false)
})

test('a failed install reports it and leaves the choice to the user', async () => {
  const server = await feed(() => ({ version: '0.2.0', package: PKG }))
  const h = harness(server, null, { install: async () => { throw new Error('read-only') } })
  await h.updater.check(false)
  await h.updater.install('now')
  assert.deepEqual([h.updater.snapshot().phase, h.updater.snapshot().error], ['failed', 'install'])
  assert.equal(h.restarts, 0)
})

test('a package manager installation points to the release page instead of installing', () => {
  const h = harness({ origin: new URL('http://127.0.0.1:1') } as TestServer, null, { external: true })
  assert.equal(h.updater.snapshot().external, true)
})

test('the AppImage is replaced atomically and only by an executable', () => {
  const d = dir()
  const target = join(d, 'Mnema.AppImage')
  writeFileSync(target, 'old', { mode: 0o755 })
  assert.throws(() => replaceAppImage(target, Buffer.from('#!/bin/sh')), /not an executable/)
  assert.throws(() => replaceAppImage(join(d, 'missing.AppImage'), PKG), /not where it was started from/)
  replaceAppImage(target, PKG)
  assert.deepEqual(readFileSync(target), PKG)
  assert.equal(statSync(target).mode & 0o777, 0o755)
  assert.equal(existsSync(`${target}.new`), false)
})

const DEB = Buffer.concat([Buffer.from('!<arch>\n'), Buffer.from('deb payload')])

test('the .deb install reads only its own feed entry, verifies it and refuses a downgrade', async () => {
  const key = 'linux-x86_64-deb'
  const server = await feed(() => ({ version: '0.2.0', package: DEB, platform: key }))
  const appImage = harness(server, null)
  await appImage.updater.check(false)
  assert.equal(appImage.updater.snapshot().phase, 'idle', 'the AppImage client ignores the -deb entry')
  const h = harness(server, null, { platform: key })
  await h.updater.check(false)
  await h.updater.install('now')
  assert.deepEqual(h.installed[0]?.bytes, DEB)
  // Tampered package and an old signed version under a new number: never installed.
  for (const signature of [minisignSignature(signer, Buffer.from('other'), '0.2.0'), minisignSignature(signer, DEB, '0.1.5')]) {
    const bad = harness(await feed(() => ({ version: '0.2.0', package: DEB, platform: key, signature })), null, { platform: key })
    await bad.updater.check(false)
    await bad.updater.install('now')
    assert.equal(bad.updater.snapshot().error, 'verify')
    assert.equal(bad.installed.length, 0)
  }
  // Same or older version than the running one is not offered.
  const same = harness(await feed(() => ({ version: '0.1.0', package: DEB, platform: key })), null, { platform: key })
  await same.updater.check(false)
  assert.equal(same.updater.snapshot().phase, 'idle')
})

test('a handoff to the software center neither restarts nor reports a failure', async () => {
  const server = await feed(() => ({ version: '0.2.0', package: DEB }))
  const h = harness(server, null, { install: () => Promise.resolve('handoff' as const) })
  await h.updater.check(false)
  await h.updater.install('now')
  assert.equal(h.restarts, 0)
  assert.equal(h.updater.snapshot().phase, 'available')
})

test('a kept package asks before it is installed at start', async () => {
  const server = await feed(() => ({ version: '0.2.0', package: DEB }))
  const d = dir()
  const first = harness(server, d)
  await first.updater.check(false)
  await first.updater.install('later')
  const declined = harness(server, d, { confirmStart: () => Promise.resolve(false) })
  assert.equal(await declined.updater.applyPending(), false)
  assert.equal(declined.installed.length, 0)
  assert.ok(declined.updater.hasPending())
  const accepted = harness(server, d, { confirmStart: () => Promise.resolve(true) })
  assert.equal(await accepted.updater.applyPending(), true)
  assert.equal(accepted.installed.length, 1)
})
