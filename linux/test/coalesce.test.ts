/** The rail's pushes are coalesced: an import of many servers is a handful of pushes, not one per change. */
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { Coalesce } from '../src/core/coalesce.ts'

test('twenty thousand changes in a row are one run, and a change after it runs schedules the next', () => {
  const timers: (() => void)[] = []
  let runs = 0
  const coalesce = new Coalesce(() => { runs++ }, 150, (run) => timers.push(run))
  // An import of 20 000 servers: each one starts, twice over.
  for (let n = 0; n < 40_000; n++) coalesce.request()
  assert.equal(timers.length, 1)
  timers.shift()!()
  assert.equal(runs, 1)
  coalesce.request()
  coalesce.request()
  assert.equal(timers.length, 1)
  timers.shift()!()
  assert.equal(runs, 2)
})

test('with real timers the changes of a short while are one push', async () => {
  let runs = 0
  const coalesce = new Coalesce(() => { runs++ }, 20)
  for (let n = 0; n < 1000; n++) coalesce.request()
  await new Promise((resolve) => setTimeout(resolve, 60))
  assert.equal(runs, 1)
})
