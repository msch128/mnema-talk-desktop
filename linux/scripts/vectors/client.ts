import { appPath, externalAllowed, inviteCode, parseAddress, parseHealth } from '../../src/core/client.ts'
import { formatVersion } from '../../src/core/version.ts'
import { type Json } from './common.ts'

export function clientVectors(): Json {
  const addresses = ['community.example', 'https://community.example/', 'https://community.example:8443', ' community.example ', 'COMMUNITY.example', 'http://localhost:8080', 'http://127.0.0.1', 'http://127.1', 'http://[::1]:3000',
    'http://LOCALHOST', '', 'http://community.example', 'http://127.0.0.1.example', 'http://192.168.1.10:8080', 'https://user@community.example', 'https://user:pw@community.example', 'https://community.example/path',
    'https://community.example:8443/?invite=code', 'http://community.example:8443/path?invite=code', 'http://localhost:8080/path?x=1',
    'https://community.example?token=x', 'https://community.example/#x', 'https://community.example\\other', 'https://community.example\n', 'https://community.example/?', 'https://a@b.example', 'ftp://community.example',
    'https://', 'https://exa mple.com', 'http://0.0.0.0', 'http://[::]', 'https://community.example:99999', '<https://community.example:8443/>', '"https://community.example"',
    'https://community.example/@alice', 'https://community.example/?mail=a@b.example', 'https://community.example/#@x', 'community.example/@alice', 'https://user@community.example/@alice']
    .map((input) => {
      try { return { input, href: parseAddress(input).href } } catch (error) { return { input, error: (error as Error).message } }
    })
  const healths = ['{"status":"ok","version":"0.7.17","web_client_api":1}', '{"status":"ok","version":"dev","web_client_api":3}', '{"status":"ok","version":"0.7.0"}', '{"status":"ok","version":"0.7.0","web_client_api":0}',
    'not JSON', '[]', '{"status":"ok","version":null,"web_client_api":1}', '{"status":"unavailable","web_client_api":1}', '{"status":"ok","web_client_api":2}', '{"status":"ok","version":"0.7.0","web_client_api":"1"}',
    '{"status":"ok","version":"0.7.0","web_client_api":null}', '{"status":"ok","version":"0.7.0","web_client_api":-1}', '{"status":"down"}', '{}']
    .map((input) => {
      try {
        const info = parseHealth(Buffer.from(input))
        return { input, ok: { version: info.version === null ? null : formatVersion(info.version), api: info.webClientApi } }
      } catch (error) { return { input, error: (error as Error).message } }
    })
  const externals = ['https://example.invalid/page?q=value', 'file:///tmp/example', 'javascript:alert(1)', 'mailto:user@example.invalid', 'tel:+4930123456', 'https://user:password@example.invalid/', 'http://example.invalid/', 'https://example.invalid/' + 'a'.repeat(5000)]
    .map((url) => ({ url, allowed: externalAllowed(new URL(url)) }))
  const ID = '11111111-1111-1111-1111-111111111111'
  const paths = ['', '/', '/index.html', '/login', '/admin', '/admin/users', '/admin/channels', '/admin/invites', '/admin/media', '/admin/system', '/admin/roles', '/admin/onboarding', '/admin/access', '/admin/emoji', '/admin/reports', '/admin/automod', '/admin/bans', '/admin/bots', '/admin/import', '/admin/audit', '/admin/status', '/settings', '/settings/audio', '/settings/app', '/settings/signin', '/settings/sessions', '/settings/privacy', '/settings/roles', '/settings/data', '/wiki', '/wiki/search', '/wiki/new', '/wiki/deleted', '/wiki/release-notes', '/wiki/release-notes/history', '/wiki/release-notes/edit', `/c/${ID}`, `/c/${ID}/m/${ID}`, `/c/${ID}/t/${ID}`, `/v/${ID}`, `/v/${ID}/chat`,
    `/v/${ID}/chat/m/${ID}`, '/c/general', '/c/general-2', `/c/general/m/${ID}`, `/c/general/t/${ID}`, '/c/forum/p/release-notes-1a2b3c4d', '/v/lounge', '/v/lounge/chat', `/v/lounge/chat/m/${ID}`, '/dm/alice', '/dm/a.b_c-d', `/dm/alice/m/${ID}`, '/dm/g/1a2b3c4d', `/dm/g/1a2b3c4d/t/${ID}`, '/other.html', '/native-media.html', '/admin/unknown', '/settings/unknown', '/settings/audio/extra', '/wiki/Not-A-Slug', '/wiki/release-notes/delete', '/wiki/release-notes/edit/extra', '/wiki/', '/admin/users/extra', '/c/Not-A-Slug', '/c/under_score', '/c/', `/c/${'a'.repeat(81)}`, '/c/general/p', '/c/general/p/Bad', '/c/general/m/not-an-id', '/v/lounge/chat/t/x', '/dm', '/dm/', '/dm/al%2fice', '/dm/alice/extra', '/dm/g/', '/dm/g/Group', `/dm/${'a'.repeat(65)}`, `/c/${ID}/`, `/c/${ID}/m`, `/v/${ID}/chat/t/${ID}`,
    `/c/${ID}%2fm%2f${ID}`, `//c/${ID}`, '/index.html/extra', '/INDEX.HTML', '/c/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA', '/c/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', `/${'a'.repeat(201)}`, '/admin/', '/login/',
    '/invite/abc234xyz789', '/invite/Team_Night-2026', '/invite/abc234xyz789/', '/invite', '/invite/', '/invite/abc', '/invite/abc234xyz789/extra', '/INVITE/abc234xyz789', `/invite/${'a'.repeat(65)}`]
    .map((path) => ({ path, ok: appPath(path) }))
  const invites = [
    'https://community.example:8443/?invite=abc234xyz789', 'http://community.example:8443/path?invite=abc234xyz789',
    'https://community.example/invite/abc234xyz789', 'https://community.example/invite/Team_Night-2026/',
    'https://community.example/#/join?invite=abc234xyz789', 'https://community.example:8443/?invite=abc234xyz789.',
    '<https://community.example:8443/?invite=abc234xyz789>', 'community.example:8443',
    'https://community.example/?invite=short', 'https://community.example/invite/nope',
    'https://user@community.example/?invite=abc234xyz789',
  ].map((input) => ({ input, code: inviteCode(input) }))
  return { addresses, healths, externals, app_paths: paths, invites }
}
