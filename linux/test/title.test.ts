import assert from 'node:assert/strict'
import { test } from 'node:test'
import { TITLE, windowTitle } from '../src/core/title.ts'

test('the window is titled with the front server in front of the product', () => {
  const instances = [{ id: 'a', name: 'Lantern Fox' }, { id: 'b', name: 'b.example' }]
  assert.equal(windowTitle({ active: null, instances }), TITLE)
  assert.equal(windowTitle({ active: 'b', instances }), `b.example — ${TITLE}`)
  assert.equal(windowTitle({ active: 'gone', instances }), TITLE)
})
