/**
 * The extras of the rail on the Electron host: muting a server, taking the
 * list out and in, invite links, and the checks of what the page may ask for
 * them.
 */
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, test } from 'node:test'
import { MAX_INSTANCES, Registry, emptyStored, idFor, loadStored, saveStored, storedFrom, storedText } from '../src/core/instances.ts'
import { inviteChoice } from '../src/core/invite.ts'
import { serverMenuAction } from '../src/core/notify.ts'
import { checkDialogResult, dialogHeight, exportTarget, parseDialogSpec, parseEdit } from '../src/core/railui.ts'

const dirs: string[] = []
afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }) })
const tmp = () => { const d = mkdtempSync(join(tmpdir(), 'mnema-extras-')); dirs.push(d); return d }

const url = (host: string) => new URL(`https://${host}/`)
const id = (host: string) => idFor(url(host))
function registry(...hosts: string[]): Registry {
  const r = new Registry()
  for (const h of hosts) r.add(url(h))
  return r
}
const NOW = 1_800_000_000
const HOUR = 3600

// ------------------------------------------------------------------- mute

test('a server is muted for a while, for ever, or not at all, and a mute ends by itself', () => {
  const r = registry('a.example', 'b.example')
  const a = id('a.example')
  assert.equal(r.isMuted(a, NOW), false)
  assert.equal(r.mute(a, NOW + HOUR, NOW), true)
  assert.equal(r.isMuted(a, NOW + HOUR - 1), true)
  assert.equal(r.isMuted(a, NOW + HOUR), false)
  assert.equal(r.rail(true, NOW).instances.find((i) => i.id === a)?.muted_until, NOW + HOUR)
  assert.equal(r.rail(true, NOW + HOUR).instances.find((i) => i.id === a)?.muted_until, null)
  assert.equal(r.mute(a, -1, NOW), true)
  assert.equal(r.rail(true, NOW + 10 * 365 * 24 * HOUR).instances[0]?.muted_until, -1)
  assert.equal(r.mute(a, 0, NOW), true)
  assert.equal(r.isMuted(a, NOW), false)
  assert.equal(r.rail(true, NOW).instances[1]?.muted_until, null)
})

test('a mute in the past, too far ahead, for an unknown server or not a number is refused', () => {
  const r = registry('a.example')
  const a = id('a.example')
  for (const until of [NOW, NOW - 5, NOW + 367 * 24 * HOUR, -2, Number.NaN, Number.POSITIVE_INFINITY]) assert.equal(r.mute(a, until, NOW), false, String(until))
  assert.equal(r.mute('0123456789abcdef', NOW + HOUR, NOW), false)
  assert.equal(r.isMuted(a, NOW), false)
})

test('the mentions of a muted server stay out of the launcher badge, its own counts stay', () => {
  const r = registry('a.example', 'b.example')
  r.report(id('a.example'), 2, 3)
  r.report(id('b.example'), 1, 4)
  assert.equal(r.badge(NOW), 7)
  r.mute(id('a.example'), NOW + HOUR, NOW)
  assert.equal(r.badge(NOW), 4)
  assert.equal(r.rail(true, NOW).instances[0]?.mentions, 3)
  assert.equal(r.badge(NOW + HOUR), 7)
})

test('mutes are kept in the list, only live ones, and come back with it', () => {
  const dir = tmp()
  const file = join(dir, 'instances.json')
  const r = registry('a.example', 'b.example', 'c.example')
  const future = Math.floor(Date.now() / 1000) + 3 * HOUR
  r.mute(id('a.example'), future)
  r.mute(id('b.example'), -1)
  saveStored(file, r.stored())
  const text = readFileSync(file, 'utf8')
  assert.deepEqual(JSON.parse(text).muted, { [id('a.example')]: future, [id('b.example')]: -1 })
  const again = Registry.restore(loadStored(file))
  assert.equal(again.isMuted(id('a.example')), true)
  assert.equal(again.isMuted(id('b.example')), true)
  assert.equal(again.isMuted(id('c.example')), false)
  // Old, unknown and odd entries do not survive a load.
  const odd = Registry.restore({ ...emptyStored(), servers: [url('a.example')], muted: { [id('a.example')]: 5, [id('z.example')]: -1, nonsense: 'x' } })
  assert.equal(odd.isMuted(id('a.example')), false)
  assert.deepEqual(odd.stored().muted, {})
})

test('a list from before the extras loads without mutes', () => {
  const stored = storedFrom({ version: 1, servers: ['https://a.example/'], active: null })
  assert.ok(stored)
  assert.equal(Registry.restore(stored).isMuted(id('a.example')), false)
})

test('the page asks for a mute, a mark as read, an export and an import in plain arguments', () => {
  const a = id('a.example')
  assert.deepEqual(parseEdit({ op: 'mute', id: a, until: -1 }), { op: 'mute', id: a, until: -1 })
  assert.deepEqual(parseEdit({ op: 'mute', id: a, until: 0 }), { op: 'mute', id: a, until: 0 })
  assert.deepEqual(parseEdit({ op: 'mark_read', id: a }), { op: 'mark_read', id: a })
  assert.deepEqual(parseEdit({ op: 'export' }), { op: 'export' })
  assert.deepEqual(parseEdit({ op: 'import', name: '{}' }), { op: 'import', text: '{}' })
  for (const bad of [{ op: 'mute', id: a }, { op: 'mute', id: a, until: '5' }, { op: 'mute', id: a, until: -2 }, { op: 'mute', id: a, until: 1.5 }, { op: 'mute', id: 'x', until: 5 }, { op: 'mark_read', id: 'f-00000000' }, { op: 'import' }, { op: 'import', name: 'x'.repeat(65537) }]) {
    assert.throws(() => parseEdit(bad), JSON.stringify(bad).slice(0, 60))
  }
})

test('the Servers menu has the quick switcher with Ctrl+K, and Ctrl+1 to 9 still lead to the n-th server', () => {
  assert.deepEqual(serverMenuAction('server-switch'), { kind: 'switch' })
  assert.deepEqual(serverMenuAction('server-3'), { kind: 'nth', number: 3 })
  const source = readFileSync(new URL('../src/main/menu.ts', import.meta.url), 'utf8')
  assert.match(source, /id: 'server-switch'[^\n]*accelerator: 'CmdOrCtrl\+K'/)
  assert.match(source, /quickSwitch\(ctx\)/)
})

test('Ctrl+N counts the servers in the order of the rail, folders opened up', () => {
  const r = registry('a.example', 'b.example', 'c.example')
  assert.equal(r.edit({ op: 'new_folder', id: id('a.example') }, 'f-00000001'), true)
  assert.equal(r.edit({ op: 'place', id: id('c.example'), target: 'f-00000001', before: id('a.example') }, 'f-00000002'), true)
  // Folder [c, a], then b.
  assert.equal(r.activateNth(1), id('c.example'))
  assert.equal(r.activateNth(2), id('a.example'))
  assert.equal(r.activateNth(3), id('b.example'))
})

test('a muted server shows no toast and the badge leaves it out', () => {
  const notify = readFileSync(new URL('../src/main/notify.ts', import.meta.url), 'utf8')
  assert.match(notify, /registry\.isMuted\(server\)[^\n]*return null/)
  assert.match(readFileSync(new URL('../src/main/shell.ts', import.meta.url), 'utf8'), /setLauncherBadge\(ctx\.trusted\.registry\.badge\(\)\)/)
})

test('marking a server read tells its own page, and only a running one', () => {
  const shell = readFileSync(new URL('../src/main/shell.ts', import.meta.url), 'utf8')
  const body = /function markRead\([\s\S]*?\n\}/.exec(shell)?.[0] ?? ''
  assert.match(body, /registry\.session\(id\)/)
  assert.match(body, /ctx\.shell\.pages\.get\(id\)/)
  assert.ok(body.includes("window.dispatchEvent(new CustomEvent('mnema-desktop:mark-all-read'))"))
  assert.doesNotMatch(body, /\$\{/)
})

// ------------------------------------------------------------- dialog

test('a dialog may hold a list or a file to choose, one of them, and is answered within it', () => {
  const buttons = [{ id: 'cancel', text: 'Cancel' }]
  const list = parseDialogSpec(JSON.stringify({ title: 'Switch', list: { label: 'Server', choices: [{ id: 'a1', text: 'One', hint: 'x' }, { id: 'b2', text: 'Two' }] }, buttons, cancel: 'cancel' }))
  assert.equal(dialogHeight(list), 190 + 200)
  assert.equal(checkDialogResult(list, { button: 'cancel', value: 'b2' }).value, 'b2')
  for (const value of ['c3', '', 5]) assert.equal(checkDialogResult(list, { button: 'cancel', value }).button, 'cancel')
  assert.equal(checkDialogResult(list, { button: 'cancel', value: 'zz' }).value, null)
  const file = parseDialogSpec(JSON.stringify({ title: 'Import', file: { label: 'File', accept: '.json', max: 100 }, buttons, cancel: 'cancel' }))
  assert.equal(dialogHeight(file), 190 + 90)
  assert.equal(checkDialogResult(file, { button: 'cancel', value: '{"a":1}' }).value, '{"a":1}')
  assert.equal(checkDialogResult(file, { button: 'cancel', value: 'x'.repeat(101) }).value, null)
  assert.equal(checkDialogResult(file, { button: 'cancel', value: '\u00e4'.repeat(51) }).value, null)
  assert.equal(checkDialogResult(file, { button: 'cancel', value: 'bad \ud800' }).value, null)
})

test('a list or file dialog that is not what the rail may ask is refused', () => {
  const buttons = [{ id: 'cancel', text: 'Cancel' }]
  const base = { title: 'T', buttons, cancel: 'cancel' }
  const choice = (n: number) => ({ id: `c${n}`, text: 't' })
  const bad = [
    { ...base, list: { label: 'l', choices: [] } },
    { ...base, list: { label: 'l', choices: Array.from({ length: 13 }, (_, n) => choice(n)) } },
    { ...base, list: { label: 'l', choices: [{ id: 'A B', text: 't' }] } },
    { ...base, list: { label: 'l', choices: [choice(1), choice(1)] } },
    { ...base, list: { label: 'l', choices: [{ id: 'a', text: 'x'.repeat(101) }] } },
    { ...base, input: { label: 'n', value: '', max: 5 }, list: { label: 'l', choices: [choice(1)] } },
    { ...base, file: { label: 'f', accept: '.exe', max: 10 } },
    { ...base, file: { label: 'f', accept: '.json', max: 70000 } },
    { ...base, file: { label: 'f', accept: '.json', max: 0 } },
    { ...base, input: { label: 'n', value: '', max: 5 }, file: { label: 'f', accept: '.json', max: 10 } },
  ]
  for (const spec of bad) assert.throws(() => parseDialogSpec(JSON.stringify(spec)), JSON.stringify(spec).slice(0, 80))
})

// ------------------------------------------------------- export and import

const exported = (r: Registry): string => storedText(r.stored())

test('the exported list holds addresses, names, layout and mutes and nothing else', () => {
  const r = registry('a.example', 'b.example')
  r.edit({ op: 'rename', id: id('a.example'), name: 'Home' }, 'f-00000001')
  r.mute(id('b.example'), -1)
  r.report(id('a.example'), 4, 4)
  const doc = JSON.parse(exported(r)) as Record<string, unknown>
  assert.deepEqual(Object.keys(doc).sort(), ['active', 'aliases', 'items', 'muted', 'servers', 'version'])
  assert.deepEqual(doc['servers'], ['https://a.example/', 'https://b.example/'])
})

test('an exported list taken into an empty registry gives the same servers, names, layout and mutes', () => {
  const r = registry('a.example', 'b.example', 'c.example')
  r.edit({ op: 'group', id: id('c.example'), target: id('a.example') }, 'f-0000000a')
  r.edit({ op: 'folder_edit', id: 'f-0000000a', name: 'Work', color: '#12ab34' }, 'f-0000000b')
  r.edit({ op: 'rename', id: id('b.example'), name: 'Home' }, 'f-0000000b')
  r.mute(id('c.example'), -1)
  const text = exported(r)
  const target = new Registry()
  const stored = storedFrom(JSON.parse(text))
  assert.ok(stored)
  const added = target.merge(stored)
  assert.equal(added.length, 3)
  assert.equal(JSON.stringify(target.rail(true).items), JSON.stringify(r.rail(true).items))
  assert.deepEqual(target.rail(true).instances.map((i) => [i.id, i.name, i.muted_until]), r.rail(true).instances.map((i) => [i.id, i.name, i.muted_until]))
  assert.equal(target.active(), null)
})

test('an import adds only what is missing, keeps the front server, and puts the new ones in the file\'s order', () => {
  const r = registry('a.example', 'b.example')
  r.activate(id('b.example'))
  const other = registry('c.example', 'a.example')
  other.edit({ op: 'rename', id: id('a.example'), name: 'Mine' }, 'f-00000001')
  const stored = storedFrom(JSON.parse(exported(other)))
  assert.ok(stored)
  const added = r.merge(stored)
  assert.deepEqual(added.map((s) => s.id), [id('c.example')])
  assert.equal(r.active(), id('b.example'))
  assert.deepEqual(r.ids(), [id('c.example'), id('a.example'), id('b.example')])
  assert.equal(r.rail(true).instances.find((i) => i.id === id('a.example'))?.alias, 'Mine')
})

test('an import without a layout keeps the folders that are here', () => {
  const r = registry('a.example', 'b.example')
  r.edit({ op: 'group', id: id('b.example'), target: id('a.example') }, 'f-00000001')
  const stored = storedFrom({ version: 1, servers: ['https://c.example/'], active: null })
  assert.ok(stored)
  r.merge(stored)
  const items = r.rail(true).items
  assert.equal(items[0]?.kind, 'folder')
  assert.equal(items.length, 2)
})

test('a hostile list is cleaned or refused, and never starts more than the limit', () => {
  assert.equal(storedFrom('nope'), null)
  assert.equal(storedFrom([]), null)
  assert.equal(storedFrom({ version: 2, servers: [] }), null)
  assert.equal(storedFrom({ version: 1, servers: 'x' }), null)
  const servers = [...Array.from({ length: 12 }, (_, n) => `https://s${n}.example/`), 'javascript:alert(1)', 'file:///etc/passwd', 'https://s1.example/', 5, 'https://user:pw@evil.example/']
  const stored = storedFrom({ version: 1, servers, active: 'https://nowhere.example/', aliases: { [id('s0.example')]: '<img>' + 'x'.repeat(100), zz: 'no' }, items: 'rubbish', muted: [1, 2] })
  assert.ok(stored)
  assert.ok(stored.servers.length <= MAX_INSTANCES)
  assert.ok(stored.servers.every((u) => u.protocol === 'https:' && u.username === ''))
  const r = registry('keep.example')
  const added = r.merge(stored)
  assert.ok(added.length + 1 <= MAX_INSTANCES)
  assert.equal(r.length, added.length + 1)
  const alias = r.rail(true).instances.find((i) => i.id === id('s0.example'))?.alias
  assert.ok(alias === null || Array.from(alias ?? '').length <= 40)
  assert.equal(r.isMuted(id('s0.example')), false)
})

test('a list that is in the way of nothing is written to a free name in the downloads folder', () => {
  const taken = new Set(['/d/mnema-servers.json', '/d/mnema-servers (2).json'])
  assert.equal(exportTarget('/d', (p) => taken.has(p)), '/d/mnema-servers (3).json')
  assert.equal(exportTarget('/d', () => false), '/d/mnema-servers.json')
  const shell = readFileSync(new URL('../src/main/shell.ts', import.meta.url), 'utf8')
  assert.match(shell, /app\.getPath\('downloads'\)/)
  assert.match(shell, /exportTarget\(dir, existsSync\)/)
  assert.match(shell, /void startServer\(ctx, slot\.id\)/)
})

// ------------------------------------------------------------- invite links

const choice = (link: string, known: string[] = [], room = true) => inviteChoice(new URL(link), (x) => known.includes(x), room)

test('an invite link of a server that is not in the list offers to add it', () => {
  assert.equal(choice('https://new.example/invite/abcDEF123'), 'add')
  assert.equal(choice('https://new.example:8443/invite/abcDEF123'), 'add')
})

test('an invite link of a server that is here opens its join screen without asking', () => {
  assert.equal(choice('https://a.example/invite/abcDEF123', [id('a.example')]), 'known')
  assert.equal(choice('https://a.example:8443/invite/abcDEF123', [id('a.example')]), 'add')
})

test('with the list full, or for anything but an invite link of https, the link is left to the browser', () => {
  assert.equal(choice('https://new.example/invite/abcDEF123', [], false), 'external')
  assert.equal(choice('https://new.example/'), 'external')
  assert.equal(choice('https://new.example/invite/ab'), 'external')
  assert.equal(choice('https://new.example/blog/invite/abcDEF123'), 'external')
  assert.equal(choice('http://new.example/invite/abcDEF123'), 'external')
  assert.equal(choice('https://user:pw@new.example/invite/abcDEF123'), 'external')
  assert.equal(choice('https://user@new.example/invite/abcDEF123'), 'external')
})

test('a look-alike host is a different server: it is offered as itself, never taken for the one that is here', () => {
  const known = [id('mnema.example')]
  assert.equal(choice('https://mnema.example.evil.test/invite/abcDEF123', known), 'add')
  assert.equal(choice('https://mnerna.example/invite/abcDEF123', known), 'add')
  assert.equal(choice('https://xn--mnema-8ra.example/invite/abcDEF123', known), 'add')
  assert.equal(choice('https://MNEMA.example/invite/abcDEF123', known), 'known')
})

test('the link is handled by the decision before the browser question, with the host shown as the browser would', () => {
  const windows = readFileSync(new URL('../src/main/windows.ts', import.meta.url), 'utf8')
  assert.ok(windows.indexOf('offerServer(ctx, parent, url)') > windows.indexOf('if (!externalAllowed(url)) return'))
  assert.ok(windows.indexOf('offerServer(ctx, parent, url)') < windows.indexOf('...externalPrompt(url)'))
  assert.match(windows, /message: 'Add this server\?', detail: url\.host/)
  assert.match(windows, /openExternal\(parent\(\), url, ctx\)/)
  assert.match(windows, /openExternal\(parent\(\), target, ctx\)/)
})
