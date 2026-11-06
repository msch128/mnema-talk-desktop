import assert from 'node:assert/strict'
import { afterEach, mock, test } from 'node:test'
import { Retries, backoffMs, isTransportFailure } from '../src/core/retry.ts'
import { NAT_HINT } from '../src/core/discover.ts'

afterEach(() => mock.timers.reset())

test('the wait grows from five seconds to five minutes and stays there', () => {
  assert.deepEqual([0, 1, 2, 5, 6, 7, 1000].map(backoffMs), [5_000, 10_000, 20_000, 160_000, 300_000, 300_000, 300_000])
})

test('only a failure to reach the server is retried', () => {
  for (const why of [
    `The instance refused the connection. Check the address and the port. ${NAT_HINT}`,
    `The instance did not answer in time. Check the address, the port and your network. ${NAT_HINT}`,
    'The instance address could not be resolved. Check the address and your network.',
    'Unable to reach the instance using trusted HTTPS.',
    'The instance is unavailable or redirects to a different address.',
    'Instance check failed.',
    'Invalid instance response.',
  ]) assert.equal(isTransportFailure(why), true, why)
  for (const why of [
    'This instance needs an update for the desktop client.',
    'This instance is not compatible with this desktop client.',
    "This computer does not trust the instance's HTTPS certificate. Check the system clock, and that no antivirus or network filter intercepts HTTPS. (CERT_HAS_EXPIRED)",
    'This server is too old for this desktop client (needs 0.9.0 or newer). Ask the administrator to update it.',
    'The web app of this server was withdrawn. Restart the client to get a current one.',
  ]) assert.equal(isTransportFailure(why), false, why)
})

test('a server that was unreachable at start is started again after the wait, with the next attempt number', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const retries = new Retries()
  const started: number[] = []
  const record = (next: number) => { started.push(next) }
  retries.schedule('a', 0, record)
  mock.timers.tick(4_999)
  assert.deepEqual(started, [])
  mock.timers.tick(1)
  assert.deepEqual(started, [1])
  retries.schedule('a', 1, record)
  mock.timers.tick(10_000)
  assert.deepEqual(started, [1, 2])
})

test('removing or reconnecting a server cancels its waiting retry, and a new one replaces the old', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const retries = new Retries()
  const started: string[] = []
  retries.schedule('a', 0, () => started.push('a'))
  retries.cancel('a')
  retries.schedule('b', 0, () => started.push('b-old'))
  retries.schedule('b', 0, () => started.push('b-new'))
  mock.timers.tick(600_000)
  assert.deepEqual(started, ['b-new'])
})
