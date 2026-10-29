import { test } from 'node:test'
import assert from 'node:assert/strict'
import { pageShown, railShown } from '../src/core/visibility.ts'

test('only the front page is shown, and none while the window is minimised', () => {
  assert.equal(pageShown('a', 'a', false), true)
  assert.equal(pageShown('b', 'a', false), false)
  assert.equal(pageShown(null, null, false), true, 'the page for adding a server is in front when no server is')
  assert.equal(pageShown(null, 'a', false), false)
  assert.equal(pageShown('a', 'a', true), false)
  assert.equal(pageShown(null, null, true), false)
})

test('the rail goes with the window', () => {
  assert.equal(railShown(false), true)
  assert.equal(railShown(true), false)
})
