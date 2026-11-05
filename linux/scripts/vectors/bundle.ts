import { MAX_FILE, MAX_PATH, validPath } from '../../src/core/bundle.ts'
import { parseVersion } from '../../src/core/version.ts'
import { bundle, entries, entry, resigned, spec, zipEntries, zipOf, type Entry, type Spec } from '../../test/helpers/fixtures.ts'
import { keysOf, outcome, type Json } from './common.ts'

export function bundleVectors(): Json {
  const versions = ['0.7.16', '0.0.0', '1.2.3', '123456789.0.0', '', '1', '1.2', '1.2.3.4', 'v1.2.3', '01.2.3', '1.2.x', '1..3', '1.2.3-rc1', '-1.0.0', '99999999999.0.0', '+1.0.0', '1.2.3 ', ' 1.2.3']
    .map((input) => ({ input, parsed: parseVersion(input) }))
  const paths = ['index.html', 'assets/a-b.js', '\u00fcn\u00ef/x.js', 'a b/c', '', '/abs', 'a//b', 'a/./b', 'a/../b', '..', '.', 'a/', 'a\\b', 'a\0b', 'a\u007f', 'a\nb', '.hidden', 'a/.well-known/x', 'a'.repeat(MAX_PATH), 'a'.repeat(MAX_PATH + 1)]
    .map((path) => ({ path, valid: validPath(path) }))

  const base = spec('0.8.0')
  const good = entries(base)
  const edit = (change: (list: Entry[]) => void): Buffer => {
    const list = entries(base)
    change(list)
    return zipEntries(list)
  }
  const cases: { name: string, trusted: string[], zip: Buffer }[] = [
    { name: 'a bundle signed by a trusted key', trusted: keysOf(7), zip: bundle(base) },
    { name: 'a deflated bundle', trusted: keysOf(7), zip: zipOf(good.map((e) => ({ name: e.name, body: e.body, method: 8 as const }))) },
    { name: 'a bundle with a second trusted key listed first', trusted: keysOf(5, 7), zip: bundle(base) },
    { name: 'another key', trusted: keysOf(8), zip: bundle(base) },
    { name: 'no keys', trusted: [], zip: bundle(base) },
    { name: 'not a zip', trusted: keysOf(7), zip: Buffer.from('PK not a zip') },
    { name: 'a changed file', trusted: keysOf(7), zip: edit((e) => { entry(e, 'app/index.html').body = Buffer.from('<html>evil</html>'.slice(0, 16)) }) },
    { name: 'a changed file of another size', trusted: keysOf(7), zip: edit((e) => { entry(e, 'app/index.html').body = Buffer.from('<html>evil</html>') }) },
    { name: 'an extra file', trusted: keysOf(7), zip: edit((e) => { e.push({ name: 'app/evil.js', body: Buffer.from('x') }) }) },
    { name: 'a missing file', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1) }) },
    {
      name: 'a file swapped for another name', trusted: keysOf(7), zip: edit((e) => {
        e.splice(e.findIndex((x) => x.name === 'app/assets/app-1.js'), 1)
        e.push({ name: 'app/assets/other.js', body: Buffer.from('console.log(1)') })
      }),
    },
    { name: 'no manifest', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.json'), 1) }) },
    { name: 'no signature', trusted: keysOf(7), zip: edit((e) => { e.splice(e.findIndex((x) => x.name === 'manifest.sig'), 1) }) },
    { name: 'an edited manifest', trusted: keysOf(7), zip: edit((e) => { const m = entry(e, 'manifest.json'); m.body = Buffer.from(m.body.toString().replace('0.8.0', '9.9.9')) }) },
    { name: 'a signature that is not base64', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.from('***') }) },
    { name: 'a signature that is too short', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.from('AAAA') }) },
    { name: 'a signature with whitespace around it', trusted: keysOf(7), zip: edit((e) => { const s = entry(e, 'manifest.sig'); s.body = Buffer.from(`\n  ${s.body.toString()}\r\n`) }) },
    { name: 'a signature above the limit', trusted: keysOf(7), zip: edit((e) => { entry(e, 'manifest.sig').body = Buffer.alloc(2000, 0x41) }) },
  ]
  const files = (m: Record<string, unknown>) => m.files as Record<string, unknown>[]
  const resignedCases: [string, (m: Record<string, unknown>) => void, Spec?][] = [
    ['an unchanged manifest, signed again', () => {}],
    ['schema 2', (m) => { m.schema = 2 }],
    ['another name', (m) => { m.name = 'other' }],
    ['a key id that is not the signer', (m) => { m.key_id = '0000000000000000' }],
    ['a version that is not a version', (m) => { m.version = '1' }],
    ['web_client_api 0', (m) => { m.web_client_api = 0 }],
    ['a minimum server newer than the bundle', (m) => { m.min_server_version = '9.0.0' }],
    ['a floor above the bundle', (m) => { m.security_floor = '9.0.0' }],
    ['a minimum server that is not a version', (m) => { m.min_server_version = 'x' }],
    ['a floor that is not a version', (m) => { m.security_floor = '1.2' }],
    ['no files', (m) => { m.files = [] }],
    ['an unknown field', (m) => { m.surprise = 1 }],
    ['an unknown field of a file', (m) => { files(m)[0] = { ...files(m)[0], extra: 1 } }],
    ['a missing field', (m) => { delete m.commit }],
    ['a path with dot dot', (m) => { (files(m)[0] as Record<string, unknown>).path = '../x' }],
    ['a duplicated path', (m) => { (files(m)[1] as Record<string, unknown>).path = (files(m)[0] as Record<string, unknown>).path }],
    ['a hash that is not hex', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'zz' }],
    ['an upper case hash', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = 'A'.repeat(64) }],
    ['a size above the file limit', (m) => { (files(m)[0] as Record<string, unknown>).size = MAX_FILE + 1 }],
    ['a size that is not the size', (m) => { (files(m)[0] as Record<string, unknown>).size = 1 }],
    ['a hash that is not the hash', (m) => { (files(m)[0] as Record<string, unknown>).sha256 = '0'.repeat(64) }],
    ['no app shell', (m) => { (files(m)[0] as Record<string, unknown>).path = 'home.html' }],
    ['a total above the limit', (m) => {
      const list = files(m)
      for (let i = 0; i < 5; i++) list.push({ path: `big${i}`, sha256: '0'.repeat(64), size: MAX_FILE })
      for (const f of list) f.size = MAX_FILE
    }],
    ['a newer bundle with a floor and a minimum server', () => {}, spec('0.9.0', { floor: '0.8.5', minServer: '0.8.9' })],
  ]
  for (const [name, change, s] of resignedCases) cases.push({ name, trusted: keysOf(7), zip: resigned(change, s) })

  const out = cases.map(({ name, trusted: keys, zip }) => outcome(name, keys, zip))
  return { versions, paths, bundles: out }
}

