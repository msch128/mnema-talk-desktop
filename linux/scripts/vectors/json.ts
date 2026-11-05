import { parseHealth } from '../../src/core/client.ts'
import { rootKey, statementBundleKeys, statementCaps, verifyStatement } from '../../src/core/trust.ts'
import { bundleAsset } from '../../src/core/updates.ts'
import { formatVersion } from '../../src/core/version.ts'
import { entries, entry, manifestOf, signWith, spec, statementText, testKey, zipEntries } from '../../test/helpers/fixtures.ts'
import { b64, hex, keysOf, outcome, type Json } from './common.ts'

/**
 * The same document shapes fed to the four JSON readers of both clients (bundle
 * manifest, trust statement, health, release feed): a byte order mark, a
 * repeated key, an unknown field, a wrong type, data after the document. The
 * documents are kept as exact bytes (base64), since a BOM or a trailing NUL is
 * not something a JSON string can carry. The first case of each list is the
 * control and must be accepted.
 */
/** The document with `fields` (written as `{...,`) opened in front of its first member. */
export function first(doc: string, fields: string): string {
  return `${fields}${doc.slice(1)}`
}

const bom = Buffer.from([0xef, 0xbb, 0xbf])
const text = (s: string) => Buffer.from(s)

/** The document in the shapes every JSON reader of both clients must treat alike. */
const variants = (doc: string, fields: { dup: string, unknown: string }): [string, Buffer][] => [
  ['a plain document', text(doc)],
  ['spaces and newlines around the document', text(`\n \t${doc}\r\n `)],
  ['a byte order mark', Buffer.concat([bom, text(doc)])],
  ['a repeated key', text(first(doc, `{${fields.dup},`))],
  ['an unknown field', text(first(doc, `{${fields.unknown},`))],
  ['a second document', text(`${doc}${doc}`)],
  ['a character after the document', text(`${doc}x`)],
  ['a null byte after the document', Buffer.concat([text(doc), Buffer.alloc(1)])],
  ['a comma after the document', text(`${doc},`)],
  ['a trailing comma inside', text(doc.replace(/\}$/, ',}'))],
  ['single quotes', text(doc.replaceAll('"', "'"))],
  ['a comment', text(`/*x*/${doc}`)],
  ['a document that is not UTF-8', Buffer.concat([text(doc.slice(0, -1)), Buffer.from([0xff]), text('}')])],
  ['an empty input', Buffer.alloc(0)],
  ['a bare string', text('"x"')],
  ['null', text('null')],
  ['an array', text('[]')],
]

/** Manifests: signed bundles whose manifest bytes are exact. */
function manifestBundles(): Json[] {
  const base = spec('0.8.0')
  const manifest = JSON.stringify(manifestOf(base))
  const withManifest = (raw: Buffer): Buffer => {
    const list = entries(base)
    entry(list, 'manifest.json').body = raw
    entry(list, 'manifest.sig').body = Buffer.from(signWith(testKey(7), raw).toString('base64'))
    return zipEntries(list)
  }
  const edited = (change: (m: Record<string, unknown>) => void): Buffer => {
    const m = manifestOf(base)
    change(m)
    return text(JSON.stringify(m))
  }
  const typed = (key: string, value: unknown): [string, Buffer] => [`${key} as ${JSON.stringify(value)}`, edited((m) => { m[key] = value })]
  const fileField = (key: string, value: unknown): [string, Buffer] => [`a file ${key} as ${JSON.stringify(value)}`, edited((m) => { (m.files as Record<string, unknown>[])[0] = { ...(m.files as Record<string, unknown>[])[0], [key]: value } })]
  const raw = (s: string): Buffer => text(manifest.replace(/"schema":1/, s))
  const manifests: [string, Buffer][] = [
    ...variants(manifest, { dup: '"schema":1', unknown: '"surprise":1' }),
    ['a repeated key inside a file', text(manifest.replace('{"path"', '{"path":"x","path"'))],
    ['an unknown field inside a file', text(manifest.replace('{"path"', '{"extra":1,"path"'))],
    ['a repeated key with another value', raw('"schema":2,"schema":1')],
    typed('schema', '1'), typed('schema', true), typed('schema', null), typed('schema', 1.5), typed('schema', [1]),
    ['schema as 1.0', raw('"schema":1.0')], ['schema as 1e0', raw('"schema":1e0')], ['schema as 01', raw('"schema":01')], ['schema as +1', raw('"schema":+1')], ['schema as 1 with a space inside a number', raw('"schema":1 ')],
    ['schema above u32', raw('"schema":4294967297')], ['schema negative', raw('"schema":-1')], ['schema as -0', raw('"schema":-0')],
    typed('name', 5), typed('name', null), typed('version', 8), typed('commit', null), typed('key_id', 7),
    typed('web_client_api', '1'), typed('web_client_api', 1.0), typed('web_client_api', 1.5),
    typed('files', {}), typed('files', 'x'), typed('files', null),
    fileField('size', '16'), fileField('size', 16.5), fileField('size', -1), fileField('size', null), fileField('size', true), fileField('path', 5), fileField('sha256', null),
    ['a file as a string', edited((m) => { (m.files as unknown[])[0] = 'index.html' })],
    ['a file as an array', edited((m) => { (m.files as unknown[])[0] = ['index.html', 'x', 1] })],
    ['the manifest as an array of its values', text(JSON.stringify(Object.values(manifestOf(base))))],
    ['a file as an array of its values', edited((m) => { (m.files as unknown[])[0] = Object.values((m.files as Record<string, unknown>[])[0] as Record<string, unknown>) })],
    ['an escaped key', text(manifest.replace('"schema"', '"sch\\u0065ma"'))],
    ['a repeated key written two ways', text(first(manifest, '{"sch\\u0065ma":1,'))],
    ['an escaped value', text(manifest.replace('"mnema-web"', '"mnema\\u002dweb"'))],
    ['a lone surrogate', text(manifest.replace('"test"', '"\\ud800"'))],
    ['a control character in a string', Buffer.from(manifest.replace('"test"', '"te\u0001st"'))],
    ['a tab character in a string', Buffer.from(manifest.replace('"test"', '"te\tst"'))],
    ['a deeply nested unknown value', text(first(manifest, `{"surprise":${'['.repeat(300)}${']'.repeat(300)},`))],
    ['NaN', raw('"schema":NaN')], ['a number with a leading dot', raw('"schema":.5')],
  ]
  return manifests.map(([name, raw]) => outcome(name, keysOf(7), withManifest(raw)))
}

/** Trust statements: signed by the root key over the exact bytes. */
function statementVectors(rootSeed: number): Json[] {
  const stmt = statementText({ serial: 1, issued_at: '2026-10-17T10:00:00Z', expires_at: '2027-01-15T10:00:00Z', bundle_keys: [hex(1)] })
  const root = rootKey(hex(rootSeed))
  const stmtEdit = (change: (o: Record<string, unknown>) => void): Buffer => {
    const o = JSON.parse(stmt) as Record<string, unknown>
    change(o)
    return text(JSON.stringify(o))
  }
  const stmtRaw = (find: string, replace: string): Buffer => text(stmt.replace(find, replace))
  const statementInputs: [string, Buffer][] = [
    ...variants(stmt, { dup: '"serial":1', unknown: '"surprise":1' }),
    ['a repeated key with another value', stmtRaw('"serial":1', '"serial":0,"serial":1')],
    ['serial as 1.0', stmtRaw('"serial":1', '"serial":1.0')], ['serial as 1e0', stmtRaw('"serial":1', '"serial":1e0')],
    ['serial as a string', stmtEdit((o) => { o.serial = '1' })], ['serial null', stmtEdit((o) => { o.serial = null })], ['serial negative', stmtEdit((o) => { o.serial = -1 })], ['serial as -0', stmtRaw('"serial":1', '"serial":-0')],
    ['schema as a string', stmtEdit((o) => { o.schema = '1' })],
    ['issued_at as a number', stmtEdit((o) => { o.issued_at = 1 })], ['expires_at null', stmtEdit((o) => { o.expires_at = null })],
    ['bundle_keys as a string', stmtEdit((o) => { o.bundle_keys = hex(1) })], ['bundle_keys with a number', stmtEdit((o) => { o.bundle_keys = [1] })],
    ['revoked_bundle_keys null', stmtEdit((o) => { o.revoked_bundle_keys = null })],
    ['updater_keys as an object', stmtEdit((o) => { o.updater_keys = {} })],
    ['a missing field', stmtEdit((o) => { delete o.revoked_updater_keys })],
    ['max_version as an array', stmtEdit((o) => { o.max_version = [] })],
    ['max_version with a number', stmtEdit((o) => { o.max_version = { 1: 9 } })],
    ['max_version repeated api', stmtRaw('"updater_keys"', '"max_version":{"1":"0.9.0","1":"0.8.0"},"updater_keys"')],
    ['max_version twice', stmtRaw('"updater_keys"', '"max_version":{"1":"0.9.0"},"max_version":{"1":"0.8.0"},"updater_keys"')],
    ['the statement as an array of its values', text(JSON.stringify(Object.values(JSON.parse(stmt))))],
    ['an escaped key', stmtRaw('"serial"', '"ser\\u0069al"')],
  ]
  return statementInputs.map(([name, doc]) => {
    const sig = signWith(testKey(rootSeed), doc).toString('base64')
    let verify: Json
    try {
      const st = verifyStatement(doc, Buffer.from(sig), root)
      verify = { ok: { serial: st.serial, bundle_keys: statementBundleKeys(st).map((k) => k.id), caps: Object.fromEntries([...statementCaps(st)].map(([k, v]) => [String(k), formatVersion(v)])) } }
    } catch (error) {
      verify = { error: (error as Error).message }
    }
    return { name, doc: b64(doc), sig, verify }
  })
}

/** Health documents. */
function healthVectors(): Json[] {
  const health = '{"status":"ok","version":"0.7.17","web_client_api":1}'
  const healthInputs: [string, Buffer][] = [
    ...variants(health, { dup: '"status":"down"', unknown: '"surprise":[1,{"a":null}]' }),
    ['a repeated key with another value', text(health.replace('"status":"ok"', '"status":"down","status":"ok"'))],
    ['web_client_api as 1.0', text(health.replace(':1}', ':1.0}'))], ['web_client_api as 1e0', text(health.replace(':1}', ':1e0}'))],
    ['web_client_api above u32', text(health.replace(':1}', ':4294967296}'))], ['web_client_api as a string', text(health.replace(':1}', ':"1"}'))],
    ['web_client_api null', text(health.replace(':1}', ':null}'))], ['status as a number', text(health.replace('"ok"', '1'))],
    ['version as a number', text(health.replace('"0.7.17"', '17'))], ['version null', text(health.replace('"0.7.17"', 'null'))],
    ['an escaped value', text(health.replace('"ok"', '"o\\u006b"'))],
    ['the document as an array of its values', text(JSON.stringify(Object.values(JSON.parse(health))))],
    ['a repeated key inside an unknown field', text(first(health, '{"surprise":{"a":1,"a":2},'))],
    ['a repeated key inside an unknown array', text(first(health, '{"surprise":[{"a":1,"a":2}],'))],
    ...[100, 120, 125, 126, 127, 128, 129, 200].map((depth): [string, Buffer] => [`an unknown field nested ${depth} deep`, text(first(health, `{"surprise":${'['.repeat(depth)}${']'.repeat(depth)},`))]),
  ]
  return healthInputs.map(([name, bytes]) => {
    let result: Json
    try {
      const info = parseHealth(bytes)
      result = { ok: { version: info.version === null ? null : formatVersion(info.version), api: info.webClientApi } }
    } catch (error) {
      result = { error: (error as Error).message }
    }
    return { name, input: b64(bytes), result }
  })
}

/** Release feeds. */
function releaseVectors(): Json[] {
  const url = 'https://github.com/msch128/mnema-talk/releases/download/v0.8.1/mnema-web-0.8.1.zip'
  const release = `{"tag_name":"v0.8.1","assets":[{"name":"mnema-web-0.8.1.zip","browser_download_url":"${url}","size":1000}]}`
  const releaseInputs: [string, Buffer][] = [
    ...variants(release, { dup: '"tag_name":"v9.9.9"', unknown: '"draft":false' }),
    ['a repeated key with another value', text(release.replace('"tag_name":"v0.8.1"', '"tag_name":"v9.9.9","tag_name":"v0.8.1"'))],
    ['an unknown field in an asset', text(release.replace('{"name"', '{"id":7,"name"'))],
    ['a repeated key in an asset', text(release.replace('{"name"', '{"name":"x","name"'))],
    ['a size as 1000.0', text(release.replace('1000}', '1000.0}'))], ['a size as 1e3', text(release.replace('1000}', '1e3}'))],
    ['a size as a string', text(release.replace('1000}', '"1000"}'))], ['a size negative', text(release.replace('1000}', '-1}'))], ['a size null', text(release.replace('1000}', 'null}'))],
    ['no size', text(release.replace(',"size":1000', ''))],
    ['tag_name as a number', text(release.replace('"v0.8.1"', '8'))], ['tag_name null', text(release.replace('"v0.8.1"', 'null'))],
    ['assets null', text(release.replace(/"assets":\[.*\]/, '"assets":null'))], ['assets as an object', text(release.replace(/"assets":\[.*\]/, '"assets":{}'))],
    ['no assets', text('{"tag_name":"v0.8.1"}')],
    ['an asset as a string', text(release.replace(/\{"name".*\}\]/, '"x"]'))],
    ['another asset that is malformed', text(release.replace('"assets":[', '"assets":[{"name":5},'))],
    ['another asset that is fine', text(release.replace('"assets":[', '"assets":[{"name":"other","browser_download_url":"https://x.example/o","size":1},'))],
    ['an escaped value', text(release.replace('"v0.8.1"', '"v0.8.\\u0031"'))],
    ['the document as an array of its values', text(JSON.stringify(Object.values(JSON.parse(release))))],
    ['an asset as an array of its values', text(release.replace(/\{"name".*\}\]/, '["mnema-web-0.8.1.zip","' + url + '",1000]]'))],
    ['a repeated key inside an unknown field', text(first(release, '{"surprise":{"a":1,"a":2},'))],
    ['a repeated key inside an unknown field of an asset', text(release.replace('{"name"', '{"surprise":[{"a":1,"a":2}],"name"'))],
  ]
  return releaseInputs.map(([name, bytes]) => {
    const found = bundleAsset(bytes)
    return { name, input: b64(bytes), result: found === null ? null : { version: formatVersion(found.version), url: found.url.href } }
  })
}

export function jsonVectors(): Json {
  const rootSeed = 9
  return { root: hex(rootSeed), bundles: manifestBundles(), statements: statementVectors(rootSeed), healths: healthVectors(), releases: releaseVectors() }
}
