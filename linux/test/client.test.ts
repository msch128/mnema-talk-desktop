import assert from 'node:assert/strict'
import { test } from 'node:test'
import { appPath, externalAllowed, parseAddress, parseHealth } from '../src/core/client.ts'

const ID = '11111111-1111-1111-1111-111111111111'

test('health decides what the server is', () => {
  const info = parseHealth(Buffer.from('{"status":"ok","version":"0.7.17","web_client_api":1}'))
  assert.deepEqual(info, { version: [0, 7, 17], webClientApi: 1 })
  assert.deepEqual(parseHealth(Buffer.from('{"status":"ok","version":"dev","web_client_api":3}')), { version: null, webClientApi: 3 })
  for (const raw of ['{"status":"ok","version":"0.7.0"}', '{"status":"ok","version":"0.7.0","web_client_api":0}', 'not JSON', '[]', '{"status":"ok","version":null,"web_client_api":1}']) {
    assert.throws(() => parseHealth(Buffer.from(raw)), /needs an update/, raw)
  }
  assert.throws(() => parseHealth(Buffer.from('{"status":"unavailable","web_client_api":1}')), /not compatible/)
})

test('external links cannot launch files, scripts or credentialed targets', () => {
  assert.ok(externalAllowed(new URL('https://example.invalid/page?q=value')))
  for (const raw of ['file:///tmp/example', 'javascript:alert(1)', 'mailto:user@example.invalid', 'https://user:password@example.invalid/', 'http://example.invalid/']) {
    assert.ok(!externalAllowed(new URL(raw)), raw)
  }
})

test('only explicit https origins are selectable', () => {
  for (const valid of ['community.example', 'https://community.example/', 'https://community.example:8443', ' community.example ', 'http://localhost:8080', 'http://127.0.0.1', 'http://[::1]:3000']) {
    assert.doesNotThrow(() => parseAddress(valid), valid)
  }
  assert.equal(parseAddress('community.example').href, 'https://community.example/')
  for (const invalid of ['', 'http://community.example', 'http://127.0.0.1.example', 'http://192.168.1.10:8080', 'https://user@community.example', 'https://community.example/path',
    'https://community.example?token=x', 'https://community.example/#x', 'https://community.example\\other', 'https://community.example\n', 'https://community.example/?', 'https://a@b.example', 'ftp://community.example']) {
    assert.throws(() => parseAddress(invalid), Error, JSON.stringify(invalid))
  }
  assert.throws(() => parseAddress('a'.repeat(3000)), /Enter an HTTPS instance address/)
})

test('actual vue routes stay local without allowing other pages or encoded paths', () => {
  for (const path of ['', '/', '/index.html', '/login', '/admin', '/admin/users', '/admin/channels', '/admin/invites', '/admin/media', '/admin/system', `/c/${ID}`, `/c/${ID}/m/${ID}`,
    `/c/${ID}/t/${ID}`, `/v/${ID}`, `/v/${ID}/chat`, `/v/${ID}/chat/m/${ID}`]) {
    assert.ok(appPath(path), path)
  }
  for (const path of ['/other.html', '/native-media.html', '/admin/unknown', '/admin/users/extra', '/c/not-an-id', '/c/00000000-0000-0000-0000-000000000000', `/c/${ID}/`, `/c/${ID}/m`,
    `/v/${ID}/chat/t/${ID}`, `/c/${ID}%2fm%2f${ID}`, `//c/${ID}`, '/index.html/extra', '/INDEX.HTML', '/c/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA']) {
    assert.ok(!appPath(path), path)
  }
})
