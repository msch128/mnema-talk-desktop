import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

const script = (name: string) => new URL(`../scripts/${name}`, import.meta.url).pathname

test('smoke.sh and refuse.sh share one launcher, which turns the debug log on itself', () => {
  for (const name of ['smoke.sh', 'refuse.sh']) {
    const text = readFileSync(script(name), 'utf8')
    assert.match(text, /launch-lib\.sh/, name)
    assert.doesNotMatch(text, /setsid|mktemp/, name)
  }
  assert.match(readFileSync(script('launch-lib.sh'), 'utf8'), /export MNEMA_DEBUG_PROTOCOL=1/)
})

// setsid is Linux's: the scripts only ever run there (CI), and so does this check.
test('refuse.sh starts the command with the debug log on and removes its log', { skip: process.platform !== 'linux' }, () => {
  const run = spawnSync('bash', [script('refuse.sh'), '^debug=1$', 'sh', '-c', 'echo debug=$MNEMA_DEBUG_PROTOCOL; sleep 30'], { encoding: 'utf8', env: { ...process.env, MNEMA_DEBUG_PROTOCOL: '' }, timeout: 30_000 })
  assert.equal(run.status, 0, run.stdout + run.stderr)
})
