/**
 * The layout of the rail, against the vectors the Rust host reads too
 * (`test-vectors/rail-layout.json`), and the checks of what the rail page may ask.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { type Edit, Layout } from '../src/core/layout.ts'
import { checkDialogResult, dialogHeight, parseDialogSpec, parseEdit, parseMenuSpec } from '../src/core/railui.ts'

type Json = any // eslint-disable-line @typescript-eslint/no-explicit-any -- vector files are untyped JSON
const vectors: Json = JSON.parse(readFileSync(new URL('../../test-vectors/rail-layout.json', import.meta.url), 'utf8'))

function run(layout: Layout, op: Json): boolean {
  switch (op.op) {
    case 'add': return layout.add(op.id)
    case 'remove': return layout.remove(op.id)
    default: {
      const rest: Json = { ...op }
      delete rest.ok
      delete rest.folder
      return layout.apply(rest as Edit, op.folder ?? 'f-ffffffff')
    }
  }
}

for (const c of vectors.cases as Json[]) {
  test(`layout vector: ${c.name}`, () => {
    const layout = Layout.load(c.start, c.start_aliases, c.known, c.order)
    for (const op of c.ops as Json[]) assert.equal(run(layout, op), op.ok !== false, JSON.stringify(op))
    assert.deepEqual(layout.items, c.items)
    assert.deepEqual(layout.flat(), c.flat)
    assert.deepEqual(Object.fromEntries(layout.aliases), c.aliases)
  })
}

const SERVER = 'aaaaaaaaaaaaaaaa'
const OTHER = 'bbbbbbbbbbbbbbbb'

test('the page names a change in plain arguments, and the host refuses everything else', () => {
  assert.deepEqual(parseEdit({ op: 'place', id: SERVER, target: '', before: OTHER }), { op: 'place', id: SERVER, target: '', before: OTHER })
  assert.deepEqual(parseEdit({ op: 'place', id: SERVER, target: 'f-0123abcd', before: null }), { op: 'place', id: SERVER, target: 'f-0123abcd', before: '' })
  assert.deepEqual(parseEdit({ op: 'group', id: SERVER, target: OTHER, folder: 'f-ffffffff' }), { op: 'group', id: SERVER, target: OTHER })
  assert.deepEqual(parseEdit({ op: 'toggle', id: 'f-0123abcd' }), { op: 'toggle', id: 'f-0123abcd', flag: null })
  assert.deepEqual(parseEdit({ op: 'folder_edit', id: 'f-0123abcd', name: 'Work', color: '' }), { op: 'folder_edit', id: 'f-0123abcd', name: 'Work', color: null })
  assert.deepEqual(parseEdit({ op: 'step', id: 'f-0123abcd', delta: -1 }), { op: 'step', id: 'f-0123abcd', delta: -1 })
  const bad: Record<string, unknown>[] = [
    {}, { op: 'nope' }, { op: 'place', id: 'x', target: '', before: '' }, { op: 'place', id: SERVER, target: OTHER, before: '' },
    { op: 'group', id: 'f-0123abcd', target: OTHER }, { op: 'step', id: SERVER, delta: 2 }, { op: 'toggle', id: SERVER, flag: true },
    { op: 'toggle', id: 'f-0123abcd', flag: 'yes' }, { op: 'folder_edit', id: 'f-0123abcd', name: 'x', color: 'red' },
    { op: 'folder_edit', id: 'f-0123abcd', name: 'x'.repeat(500), color: null }, { op: 'rename', id: 'f-0123abcd', name: 'x' }, { op: 'rename', id: SERVER, name: 7 },
  ]
  for (const args of bad) assert.throws(() => parseEdit(args), JSON.stringify(args))
})

test('a menu is a bounded tree of plain entries', () => {
  const spec = parseMenuSpec(JSON.stringify({ token: 'm-1', items: [{ id: 'up', text: 'Move up' }, { separator: true }, { id: 'notify', text: 'Notifications', items: [{ id: 'mute', text: 'Mute', checked: true }] }] }))
  assert.equal(spec.items.length, 3)
  const bad = [
    'not json', '[]', JSON.stringify({ token: 'BAD TOKEN', items: [{ id: 'a', text: 'a' }] }), JSON.stringify({ token: 'm', items: [] }),
    JSON.stringify({ token: 'm', items: [{ id: 'a', text: 'a' }, { id: 'a', text: 'again' }] }),
    JSON.stringify({ token: 'm', items: [{ id: 'A B', text: 'a' }] }), JSON.stringify({ token: 'm', items: [{ id: 'a', text: '' }] }),
    JSON.stringify({ token: 'm', items: [{ id: 'a', text: 'x'.repeat(101) }] }),
    JSON.stringify({ token: 'm', items: [{ id: 'a', text: 'a', items: [{ id: 'b', text: 'b', items: [{ id: 'c', text: 'c' }] }] }] }),
    JSON.stringify({ token: 'm', items: [{ id: 'a', text: 'a', checked: true, items: [{ id: 'b', text: 'b' }] }] }),
    JSON.stringify({ token: 'm', items: Array.from({ length: 25 }, (_, n) => ({ id: `i${n}`, text: 't' })) }),
  ]
  for (const text of bad) assert.throws(() => parseMenuSpec(text), text.slice(0, 60))
})

const rename = {
  title: 'Rename', input: { label: 'Name', value: 'Home', max: 40 },
  colors: { label: 'Colour', value: '', choices: [{ value: '', label: 'None' }, { value: '#12ab34', label: 'Green' }] },
  buttons: [{ id: 'cancel', text: 'Cancel' }, { id: 'save', text: 'Save', primary: true }], cancel: 'cancel',
}

test('a dialog is described by the page and answered within what it offered', () => {
  const spec = parseDialogSpec(JSON.stringify(rename))
  assert.equal(dialogHeight(spec), 190 + 70 + 100)
  assert.equal(dialogHeight(parseDialogSpec(JSON.stringify({ title: 'Remove', message: 'Sure?', buttons: [{ id: 'ok', text: 'OK' }], cancel: 'ok' }))), 230)
  assert.deepEqual(checkDialogResult(spec, { button: 'save', value: 'Work', color: '#12ab34' }), { button: 'save', value: 'Work', color: '#12ab34' })
  const cancelled = { button: 'cancel', value: null, color: null }
  for (const answer of [null, 'save', { button: 'other' }, { button: 'save', value: 'x'.repeat(41) }, { button: 'save', value: 5 }, { button: 'save', color: '#000000' }]) {
    assert.deepEqual(checkDialogResult(spec, answer), cancelled, JSON.stringify(answer))
  }
  const plain = parseDialogSpec(JSON.stringify({ title: 'Remove', buttons: [{ id: 'ok', text: 'OK' }], cancel: 'ok' }))
  assert.deepEqual(checkDialogResult(plain, { button: 'ok', value: 'smuggled' }), { button: 'ok', value: null, color: null })
})

test('a dialog that is not well formed is refused', () => {
  const variants: Json[] = [
    { ...rename, buttons: [] }, { ...rename, cancel: 'nope' }, { ...rename, title: '' },
    { ...rename, input: { label: 'Name', value: 'x', max: 81 } },
    { ...rename, buttons: [{ id: 'a', text: 'A' }, { id: 'a', text: 'B' }], cancel: 'a' },
    { ...rename, colors: { label: 'c', value: '', choices: [{ value: 'red', label: 'Red' }] } },
    { ...rename, buttons: [{ id: 'Bad Id', text: 'x' }], cancel: 'Bad Id' },
  ]
  for (const variant of variants) assert.throws(() => parseDialogSpec(JSON.stringify(variant)), JSON.stringify(variant).slice(0, 80))
  assert.throws(() => parseDialogSpec('{'))
})
