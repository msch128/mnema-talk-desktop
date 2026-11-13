import assert from 'node:assert/strict'
import { rmSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { QUIET_POLLS, callOver, nowChoice, waitForCallEnd } from '../src/core/aftercall.ts'
import { SelfUpdater, type UpdaterEnv } from '../src/core/selfupdate.ts'
import { minisignKey, minisignSignature } from './helpers/fixtures.ts'
import { serve, type TestServer } from './helpers/server.ts'

const servers: TestServer[] = []
const dirs: string[] = []
afterEach(async () => {
  for (const s of servers.splice(0)) await s.close()
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

test('Update now asks first only during a Talk and ends a wait when pressed again', () => {
  for (const phase of ['available', 'ready', 'failed'] as const) {
    assert.equal(nowChoice(phase, true, false, false), 'install', phase)
    assert.equal(nowChoice(phase, true, true, false), 'ask_about_talk', phase)
    assert.equal(nowChoice(phase, true, true, true), 'install', phase)
    assert.equal(nowChoice(phase, false, false, false), 'show', phase)
  }
  assert.equal(nowChoice('waiting', true, true, false), 'stop_waiting')
  for (const phase of ['idle', 'checking', 'up_to_date', 'downloading', 'installing'] as const) assert.equal(nowChoice(phase, true, false, false), 'show', phase)
})

test('the wait ends after a few quiet polls or at once when forced', async () => {
  assert.ok(QUIET_POLLS >= 2)
  assert.ok(!callOver(QUIET_POLLS - 1, false))
  assert.ok(callOver(QUIET_POLLS, false))
  assert.ok(callOver(0, true))
  let polls = 0
  await waitForCallEnd(() => { polls += 1; return polls < 3 }, () => false, 1)
  assert.equal(polls, 2 + QUIET_POLLS, 'two polls in the Talk, then the quiet ones')
})

const signer = minisignKey(3)
const PKG = Buffer.concat([Buffer.from([0x7f, 0x45, 0x4c, 0x46]), Buffer.from('appimage payload')])

async function inTalk(): Promise<{ updater: SelfUpdater, talking: { on: boolean }, installed: string[], restarts: { n: number } }> {
  const server = await serve((r, res) => {
    if (r.url === '/latest.json') {
      const host = `http://127.0.0.1:${r.headers.host?.split(':')[1]}`
      const body = JSON.stringify({ version: '0.2.0', notes: 'n', pub_date: 'x', platforms: { 'linux-x86_64': { signature: minisignSignature(signer, PKG, '0.2.0'), url: `${host}/pkg` } } })
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(body)
      return
    }
    res.writeHead(200, { 'content-length': String(PKG.length) })
    res.end(PKG)
  })
  servers.push(server)
  const d = mkdtempSync(join(tmpdir(), 'mnema-call-'))
  dirs.push(d)
  const talking = { on: true }
  const installed: string[] = []
  const restarts = { n: 0 }
  const env: UpdaterEnv = {
    current: '0.1.0', dir: d, endpoints: [new URL('/latest.json', server.origin)], pubkey: signer.pubkey, platform: 'linux-x86_64',
    statement: () => null, installsAllowed: () => true, install: async (_bytes, version) => { installed.push(version) }, restart: () => { restarts.n += 1 },
    now: () => 1_000_000, githubOnly: false, fetchOptions: { allowHttp: true }, external: false, inTalk: () => talking.on, talkPollMs: 5,
  }
  const updater = new SelfUpdater(env)
  await updater.check(false)
  return { updater, talking, installed, restarts }
}

async function until(what: () => boolean): Promise<boolean> {
  for (let i = 0; i < 400 && !what(); i++) await new Promise((resolve) => setTimeout(resolve, 5))
  return what()
}

test('after the call the verified package waits and installs once the Talk has ended', async () => {
  const t = await inTalk()
  assert.equal(t.updater.requestNow(false), 'ask_about_talk')
  assert.equal(t.updater.snapshot().confirm_call, true)
  assert.equal(t.updater.snapshot().phase, 'available', 'nothing started yet')
  const done = t.updater.install('after_call')
  assert.ok(await until(() => t.updater.snapshot().phase === 'waiting'))
  assert.equal(t.updater.snapshot().confirm_call, false)
  await new Promise((resolve) => setTimeout(resolve, 50))
  assert.deepEqual(t.installed, [], 'the Talk goes on')
  t.talking.on = false
  await done
  assert.deepEqual(t.installed, ['0.2.0'])
  assert.equal(t.restarts.n, 1)
})

test('Update now while waiting installs without waiting, and Update now anyway at once', async () => {
  const t = await inTalk()
  const done = t.updater.install('after_call')
  assert.ok(await until(() => t.updater.snapshot().phase === 'waiting'))
  assert.equal(t.updater.requestNow(false), 'stop_waiting')
  await done
  assert.deepEqual(t.installed, ['0.2.0'])
  const u = await inTalk()
  assert.equal(u.updater.requestNow(true), 'install')
  assert.ok(await until(() => u.restarts.n === 1))
})
