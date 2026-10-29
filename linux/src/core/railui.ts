/**
 * What the rail page may ask of the host, checked: a change of the layout, a
 * native menu and a small dialog. The page is the host's own, but every
 * argument is still read as untrusted text and bounded. Free of Electron so
 * the checks can be tested.
 */
import { type Args } from './ipcargs.ts'
import { type Edit } from './layout.ts'
import { validId } from './instances.ts'

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

/** The change of layout a `rail_edit` call asks for. The folder a change makes gets its id from the host, never from the page. */
export function parseEdit(args: Args): Edit {
  const op = string(args, 'op', 24)
  switch (op) {
    case 'place': {
      const target = string({ target: args['target'] ?? '' }, 'target', 10)
      const before = string({ before: args['before'] ?? '' }, 'before', 16)
      if (target !== '' && !validFolderId(target)) throw new Error('invalid arguments')
      if (before !== '' && !validId(before) && !validFolderId(before)) throw new Error('invalid arguments')
      return { op, id: either(args, 'id'), target, before }
    }
    case 'group': return { op, id: server(args, 'id'), target: server(args, 'target') }
    case 'new_folder': return { op, id: server(args, 'id') }
    case 'ungroup': return { op, id: server(args, 'id') }
    case 'step': {
      const delta = args['delta']
      if (delta !== 1 && delta !== -1) throw new Error('invalid arguments')
      return { op, id: either(args, 'id'), delta }
    }
    case 'toggle': {
      const flag = args['flag'] ?? null
      if (flag !== null && typeof flag !== 'boolean') throw new Error('invalid arguments')
      return { op, id: folder(args, 'id'), flag }
    }
    case 'folder_edit': {
      const color = args['color'] ?? null
      if (color !== null && (typeof color !== 'string' || (color !== '' && !COLOR.test(color)))) throw new Error('invalid arguments')
      return { op, id: folder(args, 'id'), name: string(args, 'name'), color: color === '' ? null : color }
    }
    case 'folder_delete': return { op, id: folder(args, 'id') }
    case 'rename': return { op, id: server(args, 'id'), name: string(args, 'name') }
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
  buttons: DialogButton[]
  cancel: string
}
export interface DialogResult { button: string, value: string | null, color: string | null }

const BUTTON_ID = /^[a-z0-9_-]{1,24}$/

function text(value: unknown, max: number, empty = false): string {
  if (typeof value !== 'string' || value.length > max || (!empty && value === '')) throw new Error('invalid dialog')
  return value
}

/** A dialog as the page describes it, or an error. */
export function parseDialogSpec(raw: string): DialogSpec {
  let json: unknown
  try { json = JSON.parse(raw) } catch { throw new Error('invalid dialog') }
  if (!isRecord(json)) throw new Error('invalid dialog')
  const spec: DialogSpec = { title: text(json['title'], 80), buttons: [], cancel: '' }
  if (json['message'] !== undefined) spec.message = text(json['message'], 300)
  const input = json['input']
  if (input !== undefined) {
    if (!isRecord(input)) throw new Error('invalid dialog')
    const max = input['max']
    if (typeof max !== 'number' || !Number.isInteger(max) || max < 1 || max > 80) throw new Error('invalid dialog')
    const value = text(input['value'], 40, true)
    spec.input = { label: text(input['label'], 60), value, max, ...(input['placeholder'] !== undefined ? { placeholder: text(input['placeholder'], 40) } : {}) }
  }
  const colors = json['colors']
  if (colors !== undefined) {
    if (!isRecord(colors) || !Array.isArray(colors['choices']) || colors['choices'].length === 0 || colors['choices'].length > 12) throw new Error('invalid dialog')
    const choices = (colors['choices'] as unknown[]).map((choice) => {
      if (!isRecord(choice)) throw new Error('invalid dialog')
      const value = text(choice['value'], 7, true)
      if (value !== '' && !COLOR.test(value)) throw new Error('invalid dialog')
      return { value, label: text(choice['label'], 40) }
    })
    const value = text(colors['value'], 7, true)
    if (value !== '' && !COLOR.test(value)) throw new Error('invalid dialog')
    spec.colors = { label: text(colors['label'], 60), value, choices }
  }
  const buttons = json['buttons']
  if (!Array.isArray(buttons) || buttons.length < 1 || buttons.length > 3) throw new Error('invalid dialog')
  spec.buttons = (buttons as unknown[]).map((button) => {
    if (!isRecord(button)) throw new Error('invalid dialog')
    const id = text(button['id'], 24)
    if (!BUTTON_ID.test(id)) throw new Error('invalid dialog')
    return { id, text: text(button['text'], 40), ...(button['primary'] === true ? { primary: true } : {}), ...(button['danger'] === true ? { danger: true } : {}) }
  })
  if (new Set(spec.buttons.map((b) => b.id)).size !== spec.buttons.length) throw new Error('invalid dialog')
  spec.cancel = text(json['cancel'], 24)
  if (!spec.buttons.some((b) => b.id === spec.cancel)) throw new Error('invalid dialog')
  return spec
}

export const DIALOG_WIDTH = 400

/** How tall the dialog window is, from what it shows. */
export function dialogHeight(spec: DialogSpec): number {
  return 190 + (spec.input ? 70 : 0) + (spec.colors ? 100 : 0) + (spec.message ? 40 : 0)
}

export function cancelled(spec: DialogSpec): DialogResult {
  return { button: spec.cancel, value: null, color: null }
}

/** What the dialog page answered, held against what it offered: anything else counts as cancelling. */
export function checkDialogResult(spec: DialogSpec, raw: unknown): DialogResult {
  if (!isRecord(raw) || typeof raw['button'] !== 'string' || !spec.buttons.some((b) => b.id === raw['button'])) return cancelled(spec)
  const value = raw['value'] ?? null
  const color = raw['color'] ?? null
  if (value !== null && (typeof value !== 'string' || !spec.input || Array.from(value).length > spec.input.max)) return cancelled(spec)
  if (color !== null && (typeof color !== 'string' || !spec.colors?.choices.some((c) => c.value === color))) return cancelled(spec)
  return { button: raw['button'], value, color }
}
