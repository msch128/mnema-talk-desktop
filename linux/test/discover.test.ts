import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { UNAVAILABLE, discover, unreachableReason } from '../src/core/discover.ts'
import { NOT_COMPATIBLE } from '../src/core/client.ts'

const coded = (code: string): Error => Object.assign(new Error('x'), { code })

test('a refused, unresolved or slow instance is told apart', () => {
  assert.match(unreachableReason(coded('ECONNREFUSED')), /^The instance refused the connection.*NAT loopback\/hairpin/)
  assert.match(unreachableReason(coded('ENOTFOUND')), /could not be resolved/)
  assert.match(unreachableReason(new Error('connect timeout')), /did not answer in time.*NAT loopback/)
  assert.match(unreachableReason(Object.assign(new Error('x'), { name: 'TimeoutError' })), /did not answer in time/)
})

test('a certificate refusal names the OpenSSL code', () => {
  for (const code of ['CERT_HAS_EXPIRED', 'UNABLE_TO_VERIFY_LEAF_SIGNATURE', 'ERR_TLS_CERT_ALTNAME_INVALID', 'DEPTH_ZERO_SELF_SIGNED_CERT']) {
    const text = unreachableReason(coded(code))
    assert.match(text, /^This computer does not trust/)
    assert.ok(text.endsWith(`(${code})`), text)
  }
})

test('anything else keeps the generic sentence', () => {
  assert.equal(unreachableReason(new Error('boom')), 'Unable to reach the instance using trusted HTTPS.')
  assert.equal(unreachableReason('nope'), 'Unable to reach the instance using trusted HTTPS.')
})

test('an instance that accepts the connection and never answers is reported as slow, with the NAT hint', async () => {
  const silent = createServer(() => { /* never answers */ })
  await new Promise<void>((resolve) => silent.listen(0, '127.0.0.1', resolve))
  try {
    const origin = new URL(`http://127.0.0.1:${(silent.address() as AddressInfo).port}/`)
    await assert.rejects(discover(origin, 300), /did not answer in time.*NAT loopback/)
  } finally {
    silent.closeAllConnections()
    silent.close()
  }
})

/** Asks a local server that answers every request with this status and body. */
async function discovered(status: number, body: string): Promise<Awaited<ReturnType<typeof discover>>> {
  const server = createServer((_req, res) => {
    res.writeHead(status, { 'content-type': 'application/json' })
    res.end(body)
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  try {
    return await discover(new URL(`http://127.0.0.1:${(server.address() as AddressInfo).port}/`), 5000)
  } finally {
    server.closeAllConnections()
    server.close()
  }
}

const STORAGE_DOWN = '{"error":{"code":"UNAVAILABLE","message":"file storage unavailable"},"status":"unavailable","unavailable":["storage"],"version":"3.0.4","web_client_api":1,"community_name":"Lantern","features":["dm"]}'

test('a server whose storage is down still says what it is', async () => {
  const info = await discovered(503, STORAGE_DOWN)
  assert.deepEqual(info.version, [3, 0, 4])
  assert.equal(info.webClientApi, 1)
  assert.equal(info.communityName, 'Lantern')
})

test('a 503 that does not describe the server stays unavailable', async () => {
  const bodies = [
    // What a server before the change sends.
    '{"error":{"code":"UNAVAILABLE","message":"file storage unavailable"}}',
    // A healthy document is no answer under a 503.
    '{"status":"ok","version":"3.0.4","web_client_api":1}',
    '{"status":"unavailable","version":"3.0.4"}',
    '{"status":"unavailable","web_client_api":1}',
    '<html>bad gateway</html>',
    '',
    `{"status":"unavailable","version":"3.0.4","web_client_api":1,"pad":"${'x'.repeat(17 * 1024)}"}`,
  ]
  for (const body of bodies) await assert.rejects(discovered(503, body), { message: UNAVAILABLE }, body.slice(0, 80))
})

test('any other status is unavailable whatever its body says', async () => {
  for (const status of [500, 502, 404]) await assert.rejects(discovered(status, STORAGE_DOWN), { message: UNAVAILABLE }, String(status))
})

test('a 200 is read as before', async () => {
  const info = await discovered(200, '{"status":"ok","version":"3.0.4","web_client_api":1,"community_name":"Lantern"}')
  assert.deepEqual(info.version, [3, 0, 4])
  assert.equal(info.communityName, 'Lantern')
  // The degraded document is not a healthy one: under a 200 it is refused as it always was.
  await assert.rejects(discovered(200, STORAGE_DOWN), { message: NOT_COMPATIBLE })
})
