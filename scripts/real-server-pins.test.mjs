import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

// The real-server jobs of the Windows/macOS and the Linux workflow run the same
// mnema-talk release: the pins must move together.
const pin = (workflow, name) => readFileSync(new URL(`../.github/workflows/${workflow}`, import.meta.url), 'utf8').match(new RegExp(`^  ${name}: '?([^'\\s]+)`, 'm'))?.[1]

test('every real-server job pins the same server release, commit and SeaweedFS', () => {
  for (const name of ['REAL_SERVER_TAG', 'REAL_SERVER_REV', 'SEAWEEDFS_VERSION']) {
    const shared = pin('desktop-client.yml', name)
    assert.ok(shared, `${name} in desktop-client.yml`)
    assert.equal(pin('desktop-linux.yml', name), shared, name)
  }
  assert.match(pin('desktop-client.yml', 'REAL_SERVER_REV'), /^[0-9a-f]{40}$/)
})
