import assert from 'node:assert/strict'
import { test } from 'node:test'
import { appPath, externalAllowed, inviteCode, parseAddress, parseCommunityName, parseHealth } from '../src/core/client.ts'

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

test('the name a server gives itself is cleaned before it is shown', () => {
  const name = (body: string) => parseCommunityName(Buffer.from(body))
  const ok = '"status":"ok","version":"1.3.0","web_client_api":1'
  assert.equal(name(`{${ok},"community_name":"  Lantern\\n Fox "}`), 'Lantern Fox')
  assert.equal(name(`{${ok}}`), null)
  assert.equal(name(`{${ok},"community_name":"   "}`), null)
  assert.equal(name(`{${ok},"community_name":7}`), null)
  assert.equal(Array.from(name(`{${ok},"community_name":"${'x'.repeat(200)}"}`) ?? '').length, 80)
  assert.equal(name('not JSON'), null)
  assert.equal(name(`{${ok},"community_name":"a","community_name":"b"}`), null)
})

test('external links cannot launch files, scripts or credentialed targets', () => {
  assert.ok(externalAllowed(new URL('https://example.invalid/page?q=value')))
  assert.ok(externalAllowed(new URL('mailto:user@example.invalid')))
  assert.ok(externalAllowed(new URL('mailto:user@example.invalid?subject=Hi%20there')))
  assert.ok(!externalAllowed(new URL('mailto:')))
  assert.ok(!externalAllowed(new URL(`mailto:${'a'.repeat(2100)}@example.invalid`)))
  for (const raw of ['file:///tmp/example', 'javascript:alert(1)', 'tel:+4930123456', 'https://user:password@example.invalid/', 'http://example.invalid/']) {
    assert.ok(!externalAllowed(new URL(raw)), raw)
  }
})

test('an address keeps its port and drops a pasted invite link', () => {
  for (const valid of ['community.example', 'https://community.example/', 'https://community.example:8443', ' community.example ', 'http://localhost:8080', 'http://127.0.0.1', 'http://[::1]:3000']) {
    assert.doesNotThrow(() => parseAddress(valid), valid)
  }
  assert.equal(parseAddress('community.example').href, 'https://community.example/')
  assert.equal(parseAddress('https://community.example:8443/?invite=code').href, 'https://community.example:8443/')
  assert.equal(parseAddress('http://community.example:8443/path?invite=code').href, 'https://community.example:8443/')
  assert.equal(parseAddress('http://localhost:8080/path?x=1').href, 'http://localhost:8080/')
  assert.equal(parseAddress('<https://community.example:8443/>').href, 'https://community.example:8443/')
  for (const invalid of ['', 'https://user@community.example', 'https://community.example\\other', 'https://community.example\n', 'https://a@b.example', 'ftp://community.example']) {
    assert.throws(() => parseAddress(invalid), Error, JSON.stringify(invalid))
  }
  assert.throws(() => parseAddress('a'.repeat(3000)), /Enter an HTTPS instance address/)
})

test('a pasted invite opens the join screen and a bare address does not', () => {
  assert.equal(inviteCode('community.example:8443'), null)
  assert.equal(inviteCode('https://community.example:8443/?invite=abc234xyz789'), 'abc234xyz789')
  assert.equal(inviteCode('http://community.example:8443/invite/Team_Night-2026/'), 'Team_Night-2026')
  assert.equal(inviteCode('https://community.example/#/join?invite=abc234xyz789'), 'abc234xyz789')
  assert.equal(inviteCode('https://community.example:8443/?invite=abc234xyz789.'), 'abc234xyz789')
  assert.equal(inviteCode('<https://community.example/?invite=abc234xyz789>'), 'abc234xyz789')
  assert.equal(inviteCode('https://community.example/?invite=short'), null)
  assert.equal(inviteCode('https://user@community.example/?invite=abc234xyz789'), null)
  assert.equal(parseAddress('https://community.example:8443/invite/abc234xyz789').href, 'https://community.example:8443/')
})

test('actual vue routes stay local without allowing other pages or encoded paths', () => {
  for (const path of ['', '/', '/index.html', '/login', '/admin', '/admin/users', '/admin/channels', '/admin/invites', '/admin/media', '/admin/system', `/c/${ID}`, `/c/${ID}/m/${ID}`,
    `/c/${ID}/t/${ID}`, `/v/${ID}`, `/v/${ID}/chat`, `/v/${ID}/chat/m/${ID}`, '/invite/abc234xyz789', '/invite/Team_Night-2026', '/invite/abc234xyz789/',
    '/c/general', `/c/general/m/${ID}`, `/c/general/t/${ID}`, '/c/forum/p/release-notes-1a2b3c4d', '/v/lounge', '/v/lounge/chat', `/v/lounge/chat/m/${ID}`, '/dm/alice', '/dm/a.b_c-d', `/dm/alice/m/${ID}`, '/dm/g/1a2b3c4d', `/dm/g/1a2b3c4d/t/${ID}`]) {
    assert.ok(appPath(path), path)
  }
  for (const path of ['/other.html', '/native-media.html', '/admin/unknown', '/admin/users/extra', '/c/Not-A-Slug', '/c/under_score', '/c/general/p', '/c/general/p/Bad', '/c/general/m/not-an-id', '/dm', '/dm/al%2fice', '/dm/alice/extra', '/dm/g/Group', `/c/${ID}/`, `/c/${ID}/m`,
    `/v/${ID}/chat/t/${ID}`, `/c/${ID}%2fm%2f${ID}`, `//c/${ID}`, '/index.html/extra', '/INDEX.HTML', '/c/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA',
    '/invite', '/invite/', '/invite/abc', '/invite/abc234xyz789/extra', '/INVITE/abc234xyz789', `/invite/${'a'.repeat(65)}`]) {
    assert.ok(!appPath(path), path)
  }
})
