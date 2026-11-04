// The server rail (experimental/app/rail-ui), run against a fake host: the shared
// harness of the railPage*.test.ts files. The page is plain scripts served by the
// executable (and by the Linux client, unchanged), so it is tested here.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { vi } from 'vitest'

// Vitest runs from the page-tests directory.
const dir = resolve(process.cwd(), '../rail-ui')
const html = readFileSync(resolve(dir, 'index.html'), 'utf8')
// The page's scripts in the order index.html loads them; they share one scope, as in the page.
const script = [...html.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => readFileSync(resolve(dir, m[1]!), 'utf8')).join('\n')

export interface Server { id: string, name: string, original: string, alias: string | null, initial: string, hue: number, status: string, message: string | null, unread: number, mentions: number, icon: string | null, muted_until?: number | null }
export type Item = { kind: 'server', id: string } | { kind: 'folder', id: string, name: string, color: string | null, collapsed: boolean, servers: string[] }
export interface State { active: string | null, adding: boolean, can_add: boolean, instances: Server[], items?: Item[] }

export const server = (id: string, extra: Partial<Server> = {}): Server => ({ id, name: `${id}.example`, original: `${id}.example`, alias: null, initial: id[0]!.toUpperCase(), hue: 120, status: 'ready', message: null, unread: 0, mentions: 0, icon: null, ...extra })
export const folder = (id: string, servers: string[], extra: Partial<Extract<Item, { kind: 'folder' }>> = {}): Item => ({ kind: 'folder', id, name: '', color: null, collapsed: false, servers, ...extra })
export const only = (id: string): Item => ({ kind: 'server', id })
export const three: State = { active: 'b', adding: false, can_add: true, instances: [server('a'), server('b'), server('c')] }

export let invoke: ReturnType<typeof vi.fn>
export let state: State

// Parse the page into an inert template and drop the stylesheet and script elements
// the executable loads itself; they are not fetched in the test window.
function pageMarkup(): string {
  const template = document.createElement('template')
  template.innerHTML = html
  template.content.querySelectorAll('link, script').forEach((el) => el.remove())
  return template.innerHTML
}

/** Puts the page in the test window and runs its scripts; the host bridge must be set up already. */
export async function runPage(lang = 'en-US') {
  Object.defineProperty(navigator, 'language', { value: lang, configurable: true })
  document.documentElement.innerHTML = pageMarkup()
  // The page script is plain browser code; run it in the test window.
  // eslint-disable-next-line no-new-func
  new Function(script)()
  await vi.advanceTimersByTimeAsync(0)
}

export async function open(initial: State, lang = 'en-US', replies: Record<string, unknown> = {}) {
  state = initial
  invoke = vi.fn(async (cmd: string) => (cmd === 'rail_state' ? state : replies[cmd]))
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke }, configurable: true })
  await runPage(lang)
}

export const tiles = () => [...document.querySelectorAll<HTMLButtonElement>('.tile')]
export const add = () => document.getElementById('add') as HTMLButtonElement
export const key = (el: Element, k: string, init: KeyboardEventInit = {}) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...init }))
export const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args)
export const rail = () => (window as unknown as { mnemaRail: { set: (s: unknown) => void, chosen: (token: string, id: string) => void, quickSwitch: () => void } }).mnemaRail
export const live = () => document.getElementById('live')!.textContent
export const folderButtons = () => [...document.querySelectorAll<HTMLButtonElement>('.folder-btn')]

export interface MenuItem { id?: string, text?: string, disabled?: boolean, checked?: boolean, separator?: boolean, items?: MenuItem[] }
export interface Spec { token: string, items: MenuItem[] }
/** The menu the page last asked the host to show, and the position it asked for. */
export function shown(): { spec: Spec, x: number, y: number } {
  const all = calls('rail_menu') as { spec: string, x: number, y: number }[]
  const last = all[all.length - 1]!
  return { spec: JSON.parse(last.spec) as Spec, x: last.x, y: last.y }
}
export const texts = (items: MenuItem[]) => items.filter((i) => !i.separator).map((i) => i.text)
export const pick = (id: string) => {
  const { spec } = shown()
  rail().chosen(spec.token, id)
}
export const rightClick = (el: Element) => el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
export const flat = (items: MenuItem[]): MenuItem[] => items.flatMap((i) => [i, ...(i.items ? flat(i.items) : [])])

/** A host that answers the commands in `answers`; anything else gets the state or nothing. */
export function answer(answers: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in answers) {
      const value = answers[cmd]
      return typeof value === 'function' ? (value as () => unknown)() : value
    }
    return cmd === 'rail_state' ? state : undefined
  })
}
