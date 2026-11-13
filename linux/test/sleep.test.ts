/**
 * Servers without a loaded page: the counts the host keeps for them and which
 * hidden pages close, against the vectors the Tauri host reads too.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { MAX_INSTANCES } from '../src/core/instances.ts'
import { boundedSleep, counts, defaultSleep, pagesToUnload } from '../src/core/sleep.ts'

interface Vectors {
  counts: { states: unknown, unread: number, mentions: number }[]
  unload: { after_secs: number | null, keep: number, now: number, servers: { id: string, active: boolean, page: boolean, hidden_since: number | null, busy: boolean | null }[], unload: string[] }[]
}
const v = JSON.parse(readFileSync(new URL('../../test-vectors/rail-layout.json', import.meta.url), 'utf8')) as Vectors

test('the counts of a server without a page follow the shared vectors', () => {
  assert.ok(v.counts.length >= 5)
  for (const c of v.counts) assert.deepEqual(counts(c.states), { unread: c.unread, mentions: c.mentions }, JSON.stringify(c))
  assert.deepEqual(counts({ not: 'a list' }), { unread: 0, mentions: 0 })
})

test('which hidden pages close follows the shared vectors', () => {
  assert.ok(v.unload.length >= 5)
  for (const c of v.unload) {
    const servers = c.servers.map((s) => ({ id: s.id, active: s.active, page: s.page, hiddenSince: s.hidden_since, busy: s.busy }))
    assert.deepEqual(pagesToUnload({ afterSecs: c.after_secs, keep: c.keep }, c.now, servers), c.unload, JSON.stringify(c))
  }
})

test('the settings are held to their bounds', () => {
  assert.deepEqual(defaultSleep(), { afterSecs: 600, keep: 4 })
  assert.deepEqual(boundedSleep({ after_secs: 1, keep: 0 }), { afterSecs: 60, keep: 1 })
  assert.deepEqual(boundedSleep({ after_secs: 10 ** 9, keep: 10 ** 6 }), { afterSecs: 86_400, keep: MAX_INSTANCES })
  assert.deepEqual(boundedSleep({ after_secs: null, keep: 3 }), { afterSecs: null, keep: 3 })
  assert.deepEqual(boundedSleep('junk'), defaultSleep())
  assert.deepEqual(boundedSleep({ after_secs: 'x', keep: 1.5 }), defaultSleep())
})
