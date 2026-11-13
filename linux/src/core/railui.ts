/**
 * What the rail page may ask of the host, checked: a change of the layout, a
 * native menu and a small dialog. The page is the host's own, but every
 * argument is still read as untrusted text and bounded. Free of Electron so
 * the checks can be tested.
 */
import { join } from 'node:path'
import { type Args } from './ipcargs.ts'
import { type Edit } from './layout.ts'
import { MAX_IMPORT_BYTES, validId } from './instances.ts'

const FOLDER_ID = /^f-[0-9a-f]{8}$/
const COLOR = /^#[0-9a-f]{6}$/

export function validFolderId(id: string): boolean {
  return FOLDER_ID.test(id)
}

function string(args: Args, name: string, max = 200): string {
  const value = args[name]
  if (typeof value !== 'string' || value.length > max) throw new Error('invalid arguments')
  return value
}

function server(args: Args, name: string): string {
  const value = string(args, name, 16)
  if (!validId(value)) throw new Error('invalid arguments')
  return value
}

function folder(args: Args, name: string): string {
  const value = string(args, name, 10)
  if (!validFolderId(value)) throw new Error('invalid arguments')
  return value
}

/** A server or a folder. */
function either(args: Args, name: string): string {
  const value = string(args, name, 16)
  if (!validId(value) && !validFolderId(value)) throw new Error('invalid arguments')
  return value
}

/** What `rail_edit` may ask besides a change of layout: the mute of a server, marking it read, and taking the list out and in. */
export type HostEdit =
  | { op: 'mute', id: string, until: number }
  | { op: 'mark_read', id: string }
  | { op: 'export' }
  | { op: 'import', text: string }
  | { op: 'unload_after', until: number }
export type RailEdit = Edit | HostEdit

function placeEdit(args: Args): Edit {
  const target = string({ target: args['target'] ?? '' }, 'target', 10)
  const before = string({ before: args['before'] ?? '' }, 'before', 16)
  if (target !== '' && !validFolderId(target)) throw new Error('invalid arguments')
  if (before !== '' && !validId(before) && !validFolderId(before)) throw new Error('invalid arguments')
  return { op: 'place', id: either(args, 'id'), target, before }
}

function stepEdit(args: Args): Edit {
  const delta = args['delta']
  if (delta !== 1 && delta !== -1) throw new Error('invalid arguments')
  return { op: 'step', id: either(args, 'id'), delta }
}

function toggleEdit(args: Args): Edit {
  const flag = args['flag'] ?? null
  if (flag !== null && typeof flag !== 'boolean') throw new Error('invalid arguments')
  return { op: 'toggle', id: folder(args, 'id'), flag }
}

function folderEdit(args: Args): Edit {
  const color = args['color'] ?? null
  if (color !== null && (typeof color !== 'string' || (color !== '' && !COLOR.test(color)))) throw new Error('invalid arguments')
  return { op: 'folder_edit', id: folder(args, 'id'), name: string(args, 'name'), color: color === '' ? null : color }
}

function muteEdit(args: Args): HostEdit {
  const until = args['until']
  if (typeof until !== 'number' || !Number.isSafeInteger(until) || until < -1) throw new Error('invalid arguments')
  return { op: 'mute', id: server(args, 'id'), until }
}

/** The change of layout (or the other change) a `rail_edit` call asks for. The folder a change makes gets its id from the host, never from the page. */
export function parseEdit(args: Args): RailEdit {
  const op = string(args, 'op', 24)
  switch (op) {
    case 'place': return placeEdit(args)
    case 'group': return { op, id: server(args, 'id'), target: server(args, 'target') }
    case 'new_folder': return { op, id: server(args, 'id') }
    case 'ungroup': return { op, id: server(args, 'id') }
    case 'step': return stepEdit(args)
    case 'toggle': return toggleEdit(args)
    case 'folder_edit': return folderEdit(args)
    case 'folder_delete': return { op, id: folder(args, 'id') }
    case 'rename': return { op, id: server(args, 'id'), name: string(args, 'name') }
    case 'mute': return muteEdit(args)
    case 'mark_read': return { op, id: server(args, 'id') }
    case 'export': return { op }
    case 'import': return { op, text: string(args, 'text', MAX_IMPORT_BYTES) }
    // `until`: the seconds hidden after which a page is closed, -1 for never.
    case 'unload_after': {
      const until = args['until']
      if (until !== -1 && until !== 600 && until !== 3600) throw new Error('invalid arguments')
      return { op, until }
    }
    default: throw new Error('Unknown change.')
  }
}

// ------------------------------------------------------------------- menu

export interface MenuItem {
  id?: string
  text?: string
  separator?: true
  disabled?: boolean
  checked?: boolean
  items?: MenuItem[]
}

export interface MenuSpec { token: string, items: MenuItem[] }

const MENU_TOKEN = /^[a-z0-9-]{1,40}$/
const MENU_ID = /^[a-z0-9:_.-]{1,48}$/
const MAX_MENU_ITEMS = 24

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function menuItems(raw: unknown, depth: number, ids: Set<string>): MenuItem[] {
  if (!Array.isArray(raw) || raw.length === 0 || raw.length > MAX_MENU_ITEMS) throw new Error('invalid menu')
  return raw.map((entry: unknown): MenuItem => {
    if (!isRecord(entry)) throw new Error('invalid menu')
    if (entry['separator'] === true) return { separator: true }
    const { id, text, disabled, checked, items } = entry
    if (typeof id !== 'string' || !MENU_ID.test(id) || ids.has(id) || typeof text !== 'string' || text === '' || text.length > 100) throw new Error('invalid menu')
    if ((disabled !== undefined && typeof disabled !== 'boolean') || (checked !== undefined && typeof checked !== 'boolean')) throw new Error('invalid menu')
    ids.add(id)
    if (items !== undefined) {
      if (depth >= 1 || checked !== undefined) throw new Error('invalid menu')
      return { id, text, ...(disabled ? { disabled } : {}), items: menuItems(items, depth + 1, ids) }
    }
    return { id, text, ...(disabled ? { disabled } : {}), ...(checked !== undefined ? { checked } : {}) }
  })
}

/** A menu as the page describes it, or an error. */
export function parseMenuSpec(text: string): MenuSpec {
  let raw: unknown
  try { raw = JSON.parse(text) } catch { throw new Error('invalid menu') }
  if (!isRecord(raw) || typeof raw['token'] !== 'string' || !MENU_TOKEN.test(raw['token'])) throw new Error('invalid menu')
  return { token: raw['token'], items: menuItems(raw['items'], 0, new Set()) }
}

/** The ids a menu offers, submenus included. */
export function menuIds(items: readonly MenuItem[]): Set<string> {
  const ids = new Set<string>()
  for (const item of items) {
    if (item.id) ids.add(item.id)
    if (item.items) for (const id of menuIds(item.items)) ids.add(id)
  }
  return ids
}

// ----------------------------------------------------------------- dialog

export interface DialogButton { id: string, text: string, primary?: boolean, danger?: boolean }
export interface DialogSpec {
  title: string
  message?: string
  input?: { label: string, value: string, max: number, placeholder?: string }
  colors?: { label: string, value: string, choices: { value: string, label: string }[] }
  /** A filter field over a short list; the answer's `value` is the id of the one picked. */
  list?: { label: string, placeholder?: string, choices: { id: string, text: string, hint?: string }[] }
  /** A JSON file to choose; the answer's `value` is its text. */
  file?: { label: string, accept: '.json', max: number }
  buttons: DialogButton[]
  cancel: string
}
export interface DialogResult { button: string, value: string | null, color: string | null }

const BUTTON_ID = /^[a-z0-9_-]{1,24}$/
const CHOICE_ID = /^[a-z0-9-]{1,24}$/

function text(value: unknown, max: number, empty = false): string {
  if (typeof value !== 'string' || value.length > max || (!empty && value === '')) throw new Error('invalid dialog')
  return value
}

function dialogInput(input: unknown): NonNullable<DialogSpec['input']> {
  if (!isRecord(input)) throw new Error('invalid dialog')
  const max = input['max']
  if (typeof max !== 'number' || !Number.isInteger(max) || max < 1 || max > 80) throw new Error('invalid dialog')
  const value = text(input['value'], 40, true)
  return { label: text(input['label'], 60), value, max, ...(input['placeholder'] !== undefined ? { placeholder: text(input['placeholder'], 40) } : {}) }
}

/** A list is the dialog's only field. */
function dialogList(list: unknown, spec: DialogSpec): NonNullable<DialogSpec['list']> {
  if (spec.input || !isRecord(list) || !Array.isArray(list['choices']) || list['choices'].length === 0 || list['choices'].length > 12) throw new Error('invalid dialog')
  const choices = (list['choices'] as unknown[]).map((choice) => {
    if (!isRecord(choice)) throw new Error('invalid dialog')
    const id = text(choice['id'], 24)
    if (!CHOICE_ID.test(id)) throw new Error('invalid dialog')
    return { id, text: text(choice['text'], 100), ...(choice['hint'] !== undefined ? { hint: text(choice['hint'], 60) } : {}) }
  })
  if (new Set(choices.map((c) => c.id)).size !== choices.length) throw new Error('invalid dialog')
  return { label: text(list['label'], 60), ...(list['placeholder'] !== undefined ? { placeholder: text(list['placeholder'], 40) } : {}), choices }
}

/** A file to choose is the dialog's only field. */
function dialogFile(file: unknown, spec: DialogSpec): NonNullable<DialogSpec['file']> {
  if (spec.input || spec.list || !isRecord(file) || file['accept'] !== '.json') throw new Error('invalid dialog')
  const max = file['max']
  if (typeof max !== 'number' || !Number.isInteger(max) || max < 1 || max > MAX_IMPORT_BYTES) throw new Error('invalid dialog')
  return { label: text(file['label'], 60), accept: '.json', max }
}

function dialogColors(colors: unknown): NonNullable<DialogSpec['colors']> {
  if (!isRecord(colors) || !Array.isArray(colors['choices']) || colors['choices'].length === 0 || colors['choices'].length > 12) throw new Error('invalid dialog')
  const choices = (colors['choices'] as unknown[]).map((choice) => {
    if (!isRecord(choice)) throw new Error('invalid dialog')
    const value = text(choice['value'], 7, true)
    if (value !== '' && !COLOR.test(value)) throw new Error('invalid dialog')
    return { value, label: text(choice['label'], 40) }
  })
  const value = text(colors['value'], 7, true)
  if (value !== '' && !COLOR.test(value)) throw new Error('invalid dialog')
  return { label: text(colors['label'], 60), value, choices }
}

function dialogButtons(buttons: unknown): DialogButton[] {
  if (!Array.isArray(buttons) || buttons.length < 1 || buttons.length > 3) throw new Error('invalid dialog')
  return (buttons as unknown[]).map((button) => {
    if (!isRecord(button)) throw new Error('invalid dialog')
    const id = text(button['id'], 24)
    if (!BUTTON_ID.test(id)) throw new Error('invalid dialog')
    return { id, text: text(button['text'], 40), ...(button['primary'] === true ? { primary: true } : {}), ...(button['danger'] === true ? { danger: true } : {}) }
  })
}

/** A dialog as the page describes it, or an error. */
export function parseDialogSpec(raw: string): DialogSpec {
  let json: unknown
  try { json = JSON.parse(raw) } catch { throw new Error('invalid dialog') }
  if (!isRecord(json)) throw new Error('invalid dialog')
  const spec: DialogSpec = { title: text(json['title'], 80), buttons: [], cancel: '' }
  if (json['message'] !== undefined) spec.message = text(json['message'], 300)
  if (json['input'] !== undefined) spec.input = dialogInput(json['input'])
  if (json['list'] !== undefined) spec.list = dialogList(json['list'], spec)
  if (json['file'] !== undefined) spec.file = dialogFile(json['file'], spec)
  if (json['colors'] !== undefined) spec.colors = dialogColors(json['colors'])
  spec.buttons = dialogButtons(json['buttons'])
  if (new Set(spec.buttons.map((b) => b.id)).size !== spec.buttons.length) throw new Error('invalid dialog')
  spec.cancel = text(json['cancel'], 24)
  if (!spec.buttons.some((b) => b.id === spec.cancel)) throw new Error('invalid dialog')
  return spec
}

export const DIALOG_WIDTH = 400

/** How tall the dialog window is, from what it shows. */
export function dialogHeight(spec: DialogSpec): number {
  return 190 + (spec.input ? 70 : 0) + (spec.colors ? 100 : 0) + (spec.message ? 40 : 0) + (spec.list ? 200 : 0) + (spec.file ? 90 : 0)
}

export function cancelled(spec: DialogSpec): DialogResult {
  return { button: spec.cancel, value: null, color: null }
}

/** Whether the value is something the dialog could have given: text for a field, an id from its list, the text of a file within its limit. */
function checkValue(spec: DialogSpec, value: unknown): boolean {
  if (typeof value !== 'string') return false
  if (spec.input) return Array.from(value).length <= spec.input.max
  if (spec.list) return spec.list.choices.some((c) => c.id === value)
  if (spec.file) return Buffer.from(value, 'utf8').toString('utf8') === value && Buffer.byteLength(value, 'utf8') <= spec.file.max
  return false
}

/** What the dialog page answered, held against what it offered: anything else counts as cancelling. */
export function checkDialogResult(spec: DialogSpec, raw: unknown): DialogResult {
  if (!isRecord(raw) || typeof raw['button'] !== 'string' || !spec.buttons.some((b) => b.id === raw['button'])) return cancelled(spec)
  const value = raw['value'] ?? null
  const color = raw['color'] ?? null
  if (value !== null && !checkValue(spec, value)) return cancelled(spec)
  if (color !== null && (typeof color !== 'string' || !spec.colors?.choices.some((c) => c.value === color))) return cancelled(spec)
  return { button: raw['button'], value: value as string | null, color: color as string | null }
}

// ------------------------------------------------------------- export file

/** Where an exported list goes: `mnema-servers.json` in `dir`, or the first free `mnema-servers (2).json`, and so on. Never over a file that is there. */
export function exportTarget(dir: string, taken: (path: string) => boolean): string {
  for (let n = 1; n < 1000; n++) {
    const path = join(dir, n === 1 ? 'mnema-servers.json' : `mnema-servers (${n}).json`)
    if (!taken(path)) return path
  }
  throw new Error('The list could not be saved.')
}
