import assert from 'node:assert/strict'
import { test } from 'node:test'
import { unreachableReason } from '../src/core/discover.ts'

const coded = (code: string): Error => Object.assign(new Error('x'), { code })

test('a refused, unresolved or slow instance is told apart', () => {
  assert.match(unreachableReason(coded('ECONNREFUSED')), /^The instance refused the connection/)
  assert.match(unreachableReason(coded('ENOTFOUND')), /could not be resolved/)
  assert.match(unreachableReason(new Error('connect timeout')), /did not answer in time/)
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
