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
  for (const command of ['rail_state', 'rail_activate', 'rail_add', 'rail_cancel_add', 'rail_edit', 'rail_menu', 'rail_dialog', 'rail_icon', 'rail_remove', 'rail_reconnect']) assert.ok(rail.includes(`'${command}'`), command)
  assert.ok(!rail.includes("'rail_move'"), 'replaced by rail_edit')
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

test('the rail keeps focus across a push, holds a push during a drag, and announces only what the host did', () => {
  const rail = read('../assets/rail-ui/rail.js')
  const redraw = /function redraw\(\) \{[\s\S]*?\n\}/.exec(rail)?.[0] ?? ''
  // The tiles are replaced by render(): the focus is looked up again afterwards, never before.
  assert.ok(redraw.indexOf('render()') < redraw.indexOf('buttonOf(focused)'), 'the focus is restored after render')
  // A drag is not interrupted by a push: replacing the dragged tile would end it.
  assert.ok(redraw.indexOf('if (drag)') >= 0 && redraw.indexOf('if (drag)') < redraw.indexOf('render()'), 'a push waits for the drag')
  // What the host refused is announced as a failure, never as done.
  assert.match(rail, /T\.removed\([^\n]*: T\.removeFailed/)
  assert.match(rail, /if \(!ok\) say\(T\.editFailed\)/)
})

test('the rail menu is the system\'s own popup, not a piece of the rail page that a server page could cover', () => {
  const menu = read('../src/main/railui.ts')
  assert.match(menu, /Menu\.buildFromTemplate\(template\(spec\.items/)
  assert.match(menu, /menu\.popup\(/)
  assert.match(menu, /window\.mnemaRail&&window\.mnemaRail\.chosen\(/)
  // The page has no menu element of its own any more.
  assert.doesNotMatch(read('../assets/rail-ui/index.html'), /id="menu"/)
  assert.doesNotMatch(read('../assets/rail-ui/rail.js'), /getElementById\('menu'\)|\$\('menu'\)/)
  assert.match(read('../assets/rail-ui/rail.js'), /rail_menu/)
  // Only the latest menu, and only what it offered, is heard.
  assert.match(menu, /current\.token !== spec\.token \|\| !current\.ids\.has\(id\)/)
})

test('the menu, the dialog and the icon are answered to the rail alone, with their arguments checked', () => {
  const rail = read('../src/main/rail.ts')
  assert.match(rail, /case 'rail_menu': showMenu\(ctx, parseMenuSpec\(specText\(args\)\)/)
  assert.match(rail, /case 'rail_dialog': return showDialog\(ctx, parseDialogSpec\(specText\(args\)\)\)/)
  assert.match(rail, /case 'rail_edit': return railEdit\(ctx, parseEdit\(args\)\)/)
  assert.match(rail, /ctx\.trusted\.registry\.contains\(id\) \? ctx\.trusted\.icons\.dataUrl\(id\) : null/)
  assert.doesNotMatch(read('../src/main/ipc.ts'), /rail_|mnema:dialog/)
})

test('the dialog window is a page of the host: its own session, one page, one answer from its own window', () => {
  const dialog = read('../src/main/railui.ts')
  assert.match(dialog, /pagePreferences\(PARTITIONS\.dialog, dialogPreload\(\)\)/)
  assert.match(dialog, /event\.sender\.id !== current\.window\.webContents\.id/)
  assert.match(dialog, /checkDialogResult\(current\.spec, answer\)/)
  assert.match(dialog, /modal: true/)
  assert.match(read('../src/main/main.ts'), /prepare\(ctx, PARTITIONS\.dialog, 'dialog', assetHandler\('\/', assets\('rail-ui'\), HOST_PAGE_CSP\)\)/)
  assert.match(read('../src/main/windows.ts'), /case 'dialog': return dialogUi\(url\)/)
  assert.match(read('../src/core/origin.ts'), /pathname === '\/dialog\.html'/)
  const preload = read('../src/preload/dialog.cts')
  assert.match(preload, /exposeInMainWorld\('__MNEMA_DIALOG_HOST__', \{ spec, done \}\)/)
  assert.doesNotMatch(preload, /__TAURI_INTERNALS__|mnema:rail|mnema:invoke/)
  assert.doesNotMatch(read('../src/preload/preload.cts'), /__MNEMA_DIALOG_HOST__|mnema:dialog/)
  assert.doesNotMatch(read('../src/preload/rail.cts'), /mnema:dialog/)
})

test('the community icon is asked for at the server alone, and its files go with the server', () => {
  const icon = read('../src/core/icon.ts')
  assert.match(icon, /new URL\(ICON_PATH, origin\)/)
  assert.doesNotMatch(icon, /maxRedirects|followRedirect|headers\.location/i)
  const shell = read('../src/main/shell.ts')
  assert.match(shell, /trusted\.icons\.clear\(id\)/)
  const trusted = read('../src/core/trusted.ts')
  assert.match(trusted, /this\.registry\.readInFull\) this\.icons\.sweep\(/)
  // Refreshed once the server answered, without holding the page up.
  assert.match(shell, /current\.phase = \{ kind: 'ready', session \}\n {4}refreshIcon\(ctx, id\)/)
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
  assert.match(main, /could not start[\s\S]*?errorBox\(message\)[\s\S]*?app\.exit\(1\)/)
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

test('the rail edits through one command that may answer with a path or a count, and the extras stay behind the same checks', () => {
  const rail = read('../src/main/rail.ts')
  assert.match(rail, /case 'rail_edit': return railEdit\(ctx, parseEdit\(args\)\)/)
  const shell = read('../src/main/shell.ts')
  for (const op of ["case 'mute'", "case 'mark_read'", "case 'export'", "case 'import'"]) assert.ok(shell.includes(op), op)
  // The Ctrl+K switcher is the rail page's own list; the host only asks the rail to open it.
  assert.match(shell, /window\.mnemaRail&&window\.mnemaRail\.quickSwitch\(\)/)
  // A list that is taken in is checked like one read from disk, and sized before it is parsed.
  assert.match(shell, /MAX_IMPORT_BYTES[\s\S]*?storedFrom\(raw\)/)
})
