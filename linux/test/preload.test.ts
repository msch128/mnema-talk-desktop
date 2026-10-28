import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

const read = (name: string) => readFileSync(new URL(`../src/preload/${name}`, import.meta.url), 'utf8')

/** The `invoke` function of a preload, up to the end of its body. */
function invokeBody(source: string): string {
  const start = source.indexOf('async function invoke(')
  assert.ok(start >= 0)
  const end = source.indexOf('\n}\n', start)
  assert.ok(end > start)
  return source.slice(start, end)
}

// A sandboxed preload cannot import, so the checks of the two copies are kept in step by this test.
test('the invoke checks of both preloads stay the same except for channel and event channel', () => {
  const app = invokeBody(read('preload.cts'))
    .replace("      if (key === 'onEvent') input.onEvent = channelId(value)\n      else if (", '      if (')
    .replace("'mnema:invoke'", "'CHANNEL'")
  const rail = invokeBody(read('rail.cts')).replace("'mnema:rail'", "'CHANNEL'")
  assert.equal(app, rail)
})

test('convertFileSrc serves nothing', () => {
  assert.match(read('preload.cts'), /convertFileSrc\(_path: unknown\): string \{\n[^\n]*\n {4}return ''\n {2}\}/)
})
