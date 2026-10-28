import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { test } from 'node:test'

const here = (path: string) => new URL(path, import.meta.url)
const read = (path: string) => readFileSync(here(path), 'utf8')

test('the rail page is the same file as in the Tauri host', () => {
  const shared = readdirSync(here('../../experimental/app/rail-ui/')).sort()
  assert.deepEqual(readdirSync(here('../assets/rail-ui/')).sort(), shared)
  for (const name of shared) assert.equal(read(`../assets/rail-ui/${name}`), read(`../../experimental/app/rail-ui/${name}`), name)
})

test('the update page is the same file as in the Tauri host', () => {
  const shared = readdirSync(here('../../experimental/app/update-ui/')).sort()
  assert.deepEqual(readdirSync(here('../assets/update-ui/')).sort(), shared)
  for (const name of shared) assert.equal(read(`../assets/update-ui/${name}`), read(`../../experimental/app/update-ui/${name}`), name)
})

test('the rail commands answer only the rail, on their own channel', () => {
  const rail = read('../src/main/rail.ts')
  assert.match(rail, /event\.sender\.id !== rail\.webContents\.id/)
  assert.match(rail, /mainFrameUrl\(event\)/)
  assert.match(read('../src/main/handle.ts'), /frame !== event\.sender\.mainFrame/)
  for (const command of ['rail_state', 'rail_activate', 'rail_add', 'rail_cancel_add', 'rail_move', 'rail_remove', 'rail_reconnect']) assert.ok(rail.includes(`'${command}'`), command)
  // The pages of servers have no way to name them.
  const ipc = read('../src/main/ipc.ts')
  assert.doesNotMatch(ipc, /rail_/)
  assert.doesNotMatch(read('../src/preload/preload.cts'), /mnema:rail|__MNEMA_RAIL_HOST__/)
  assert.doesNotMatch(read('../src/preload/rail.cts'), /__TAURI_INTERNALS__|mnema:invoke/)
})

test('the rail page may not reach the network, and its files are served to it alone', () => {
  const main = read('../src/main/shell.ts')
  assert.match(main, /prepare\(ctx, PARTITIONS\.rail, 'rail', assetHandler\('\/', assets\('rail-ui'\), HOST_PAGE_CSP\)\)/)
  assert.match(read('../src/main/protocol.ts'), /connect-src 'none'/)
  assert.doesNotMatch(read('../src/main/protocol.ts'), /unsafe-inline|unsafe-eval/)
})

test('every server gets a session of its own, and its handler is bound to it', () => {
  const sessions = read('../src/main/sessions.ts')
  assert.match(sessions, /appHandler\('instance', ctx\.trusted, ctx\.debug, id\)/)
  assert.match(sessions, /appHandler\('attachment', ctx\.trusted, ctx\.debug, id\)/)
  const context = read('../src/main/context.ts')
  assert.match(context, /persist:inst-\$\{id\}/)
  assert.match(context, /mnema-att-\$\{id\}/)
})

test('the rail keeps focus and an open menu across a push, and announces only what the host did', () => {
  const rail = read('../assets/rail-ui/rail.js')
  const set = /function set\(next\) \{[\s\S]*?\n\}/.exec(rail)?.[0] ?? ''
  // The tiles are replaced by render(): the anchor and the focus are looked up again afterwards, never before.
  assert.ok(set.indexOf('render()') < set.indexOf('tileOf(owner)'), 'the menu anchor is resolved after render')
  assert.ok(set.indexOf('render()') < set.indexOf('tileOf(focused)'), 'the focus is restored after render')
  assert.doesNotMatch(set, /document\.body\.contains/)
  // What the host refused is announced as a failure, never as done.
  assert.match(rail, /\? T\.moved\([^\n]*: T\.moveFailed/)
  assert.match(rail, /\? T\.removed\([^\n]*: T\.removeFailed/)
})

test('the picker says so when there is nothing to share', () => {
  const picker = read('../assets/picker-ui/picker.js')
  assert.match(picker, /sources\.length === 0/)
  assert.match(picker, /empty: 'There is nothing to share\.'/)
})

test('unread reports push the rail only: the menu is rebuilt when the list changes', () => {
  const shell = read('../src/main/shell.ts')
  const body = (name: string) => new RegExp(`function ${name}\\([^)]*\\)[^{]*\\{([\\s\\S]*?)\\n\\}`).exec(shell)?.[1] ?? ''
  assert.doesNotMatch(body('pushRail'), /buildMenu/)
  assert.doesNotMatch(body('countsChanged'), /buildMenu/)
  assert.match(body('changed'), /buildMenu/)
})

test('a failed start tells the person, and a second launch raises the window of the shell', () => {
  const main = read('../src/main/main.ts')
  assert.match(main, /could not start[\s\S]*?dialog\.showErrorBox\(TITLE, message\)[\s\S]*?app\.exit\(1\)/)
  assert.match(main, /second-instance[\s\S]*?raiseWindow\(running\)/)
  assert.match(read('../src/main/shell.ts'), /function raiseWindow[\s\S]*?isMinimized\(\)\) window\.restore\(\)/)
})

test('the picker cannot leave a screen share request unanswered', () => {
  const permissions = read('../src/main/permissions.ts')
  assert.match(permissions, /chooseScreen\([^\n]*\)\.then\([^\n]*, \(\) => callback\(null\)\)/)
  assert.match(permissions, /catch \{[\s\S]*?finish\(null\)/)
})

test('the window title is defined once', () => {
  for (const file of ['main.ts', 'shell.ts', 'windows.ts']) assert.doesNotMatch(read(`../src/main/${file}`), /'Mnema Desktop DEV'/, file)
  assert.match(read('../src/core/title.ts'), /export const TITLE = 'Mnema Desktop DEV'/)
})
