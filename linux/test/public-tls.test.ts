import assert from 'node:assert/strict'
import { test } from 'node:test'
import { UNAVAILABLE, discover } from '../src/core/discover.ts'

// A real public server on a port that is not 443, with a Let's Encrypt
// certificate (badssl.com's test servers; one request per run). It is no Mnema
// instance, so discovery ends at "answered, but not healthy", never at a
// certificate or reachability sentence. Opt-in (MNEMA_TEST_PUBLIC_TLS=1, set by
// CI): it needs the internet.
const online = process.env.MNEMA_TEST_PUBLIC_TLS === '1'

test('a public certificate on a non-default port is trusted', { skip: !online }, async () => {
  await assert.rejects(discover(new URL('https://tls-v1-2.badssl.com:1012/')), { message: UNAVAILABLE })
})
