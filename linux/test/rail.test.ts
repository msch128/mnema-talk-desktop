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

test('the rail commands answer only the rail, on their own channel', () => {
  const rail = read('../src/main/rail.ts')
  assert.match(rail, /event\.sender\.id !== rail\.webContents\.id/)
  assert.match(rail, /frame !== event\.sender\.mainFrame/)
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
