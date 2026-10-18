import assert from 'node:assert/strict'
import { test } from 'node:test'
import { debuggingAccepted, debuggingRequested, sandboxAccepted, sandboxDisabled } from '../src/main/sandbox.ts'

test('a start without the browser sandbox is refused unless the user accepts it', () => {
  const switches = (...names: string[]) => (name: string) => names.includes(name)
  assert.equal(sandboxDisabled(switches(), {}), false)
  assert.equal(sandboxDisabled(switches('no-sandbox'), {}), true)
  assert.equal(sandboxDisabled(switches('disable-setuid-sandbox'), {}), true)
  assert.equal(sandboxDisabled(switches(), { ELECTRON_DISABLE_SANDBOX: '1' }), true)
  assert.equal(sandboxDisabled(switches(), { ELECTRON_DISABLE_SANDBOX: '' }), false)
  assert.equal(sandboxAccepted({ MNEMA_DESKTOP_ALLOW_NO_SANDBOX: '1' }), true)
  assert.equal(sandboxAccepted({ MNEMA_DESKTOP_ALLOW_NO_SANDBOX: '0' }), false)
  assert.equal(sandboxAccepted({}), false)
})

test('a remote debugging port is refused in a packaged app unless the user accepts it', () => {
  const switches = (...names: string[]) => (name: string) => names.includes(name)
  assert.equal(debuggingRequested(switches()), false)
  for (const name of ['remote-debugging-port', 'remote-debugging-pipe', 'remote-allow-origins', 'inspect', 'inspect-brk']) {
    assert.equal(debuggingRequested(switches(name)), true, name)
  }
  assert.equal(debuggingRequested(switches('enable-logging')), false)
  assert.equal(debuggingAccepted({ MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING: '1' }), true)
  assert.equal(debuggingAccepted({}), false)
})
