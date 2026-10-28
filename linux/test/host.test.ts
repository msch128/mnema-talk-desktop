import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { failure, wholeNumber } from '../src/core/ipcargs.ts'
import { closeSocketsWithDocument } from '../src/core/pagesockets.ts'
import { permitted } from '../src/core/permitted.ts'
import { Tickets } from '../src/core/tickets.ts'
import { orphanPartitions, wipeWithRetry } from '../src/core/wipe.ts'

const tmp = () => mkdtempSync(join(tmpdir(), 'mnema-host-'))
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

test('microphone and camera go to the app page itself, never to a frame inside it', () => {
  const url = 'mnema://localhost/'
  assert.equal(permitted('media', url, 'instance', ['audio', 'video'], true), true)
  assert.equal(permitted('media', url, 'instance', ['audio'], false), false)
  assert.equal(permitted('media', url, 'instance', ['audio', 'screen'], true), false)
  assert.equal(permitted('media', url, 'selector', ['audio'], true), false)
  assert.equal(permitted('media', 'https://evil.example/', 'instance', ['audio'], true), false)
  // Notifications and the rest do not depend on the frame.
  assert.equal(permitted('notifications', url, 'instance', [], false), true)
  assert.equal(permitted('geolocation', url, 'instance'), false)
})

test('a start that is asked for again supersedes the first', () => {
  const starts = new Tickets()
  const first = starts.begin('a')
  assert.equal(starts.current('a', first), true)
  const second = starts.begin('a')
  assert.equal(starts.current('a', first), false)
  assert.equal(starts.current('a', second), true)
  const other = starts.begin('b')
  assert.equal(starts.current('a', second), true, 'servers do not supersede one another')
  assert.equal(starts.current('b', other), true)
})

test('the sockets of a page end when its document is replaced or its renderer dies', () => {
  const closed: string[] = []
  const page = new EventEmitter()
  closeSocketsWithDocument(page as never, () => ({ closeAll: () => { closed.push('all') } }))
  page.emit('did-start-navigation', { isMainFrame: true, isSameDocument: true })
  page.emit('did-start-navigation', { isMainFrame: false, isSameDocument: false })
  assert.deepEqual(closed, [], 'a route change or a frame inside the page keeps them')
  page.emit('did-start-navigation', { isMainFrame: true, isSameDocument: false })
  assert.deepEqual(closed, ['all'])
  page.emit('render-process-gone')
  assert.deepEqual(closed, ['all', 'all'])
  // A server without a session (not ready, removed) has nothing to close.
  const bare = new EventEmitter()
  closeSocketsWithDocument(bare as never, () => undefined)
  bare.emit('render-process-gone')
})

test('only partitions of servers outside the list are orphans', () => {
  const known = new Set(['aaaaaaaaaaaaaaaa'])
  const names = ['inst-aaaaaaaaaaaaaaaa', 'inst-bbbbbbbbbbbbbbbb', 'mnema-att-bbbbbbbbbbbbbbbb', 'inst-short', 'inst-BBBBBBBBBBBBBBBB', 'mnema-rail', 'Cache']
  assert.deepEqual(orphanPartitions(names, known), ['inst-bbbbbbbbbbbbbbbb'])
})

test('wiping a directory that cannot be removed does not throw, and is tried again', async () => {
  const dir = join(tmp(), 'inst-aaaaaaaaaaaaaaaa')
  mkdirSync(dir)
  writeFileSync(join(dir, 'x'), '1')
  let tries = 0
  assert.doesNotThrow(() => wipeWithRetry(dir, { remove: () => { tries++; throw new Error('EBUSY') }, retryMs: 1, tries: 5 }))
  for (let waited = 0; tries < 5 && waited < 5000; waited += 10) await wait(10)
  await wait(30)
  assert.equal(tries, 5, 'tried again, and stopped after the last try')
  assert.ok(existsSync(dir))
})

test('a wipe removes the directory, and leaves it when the server is back', () => {
  const root = tmp()
  const gone = join(root, 'gone')
  const kept = join(root, 'kept')
  for (const dir of [gone, kept]) { mkdirSync(dir); writeFileSync(join(dir, 'x'), '1') }
  wipeWithRetry(gone)
  wipeWithRetry(kept, { stop: () => true })
  assert.equal(existsSync(gone), false)
  assert.equal(existsSync(kept), true)
})

test('command arguments are whole numbers, and a failure tells the page little', () => {
  assert.equal(wholeNumber({ to: 3 }, 'to'), 3)
  assert.equal(wholeNumber({ to: 0 }, 'to'), 0)
  for (const bad of [-1, 1.5, Number.NaN, Number.MAX_SAFE_INTEGER + 2, '1', null, undefined]) {
    assert.throws(() => wholeNumber({ to: bad }, 'to'), /invalid arguments/, String(bad))
  }
  assert.deepEqual(failure(new Error('x'.repeat(500))), { ok: false, error: 'x'.repeat(300) })
  assert.deepEqual(failure('/home/secret/path'), { ok: false, error: 'The command failed.' })
})
