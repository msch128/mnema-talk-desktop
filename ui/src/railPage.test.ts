// The server rail (experimental/app/rail-ui), run against a fake host. It is
// plain script served by the executable (and by the Linux client, unchanged),
// so it is tested here rather than through the Vue app.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Vitest runs from the ui directory.
const dir = resolve(process.cwd(), '../experimental/app/rail-ui')
const html = readFileSync(resolve(dir, 'index.html'), 'utf8')
const script = readFileSync(resolve(dir, 'rail.js'), 'utf8')

interface Server { id: string, name: string, initial: string, hue: number, status: string, message: string | null, unread: number, mentions: number }
interface State { active: string | null, adding: boolean, can_add: boolean, instances: Server[] }

const server = (id: string, extra: Partial<Server> = {}): Server => ({ id, name: `${id}.example`, initial: id[0]!.toUpperCase(), hue: 120, status: 'ready', message: null, unread: 0, mentions: 0, ...extra })
const three: State = { active: 'b', adding: false, can_add: true, instances: [server('a'), server('b'), server('c')] }

let invoke: ReturnType<typeof vi.fn>
let state: State

async function open(initial: State, lang = 'en-US') {
  state = initial
  Object.defineProperty(navigator, 'language', { value: lang, configurable: true })
  document.documentElement.innerHTML = html
    .replace(/^[\s\S]*<html[^>]*>/, '')
    .replace(/<\/html>\s*$/, '')
    .replace(/<(link|script)[^>]*>(<\/script>)?/g, '')
  invoke = vi.fn(async (cmd: string) => (cmd === 'rail_state' ? state : undefined))
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke }, configurable: true })
  // The page script is plain browser code; run it in the test window.
  // eslint-disable-next-line no-new-func
  new Function(script)()
  await vi.advanceTimersByTimeAsync(0)
}

const tiles = () => [...document.querySelectorAll<HTMLButtonElement>('.tile')]
const add = () => document.getElementById('add') as HTMLButtonElement
const menuButtons = () => [...document.querySelectorAll<HTMLButtonElement>('#menu button')]
const key = (el: Element, k: string, init: KeyboardEventInit = {}) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...init }))
const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args)

describe('server rail', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => { vi.useRealTimers(); Reflect.deleteProperty(window, '__MNEMA_RAIL_HOST__') })

  it('asks the host what to draw and shows one tile per server, the front one marked', async () => {
    await open(three)
    expect(invoke).toHaveBeenCalledWith('rail_state', undefined)
    expect(tiles().map((t) => t.textContent)).toEqual(['A', 'B', 'C'])
    expect(tiles().map((t) => t.getAttribute('aria-current'))).toEqual(['false', 'true', 'false'])
    expect(tiles()[1]!.getAttribute('aria-label')).toBe('b.example, current')
    expect(document.querySelector('li.current')).not.toBeNull()
    expect(document.getElementById('list')!.getAttribute('aria-label')).toBe('Servers')
  })

  it('shows unread as a dot and mentions as a count, bounded, and a refused server as a warning', async () => {
    await open({ ...three, instances: [server('a', { unread: 2 }), server('b', { mentions: 5, unread: 1 }), server('c', { status: 'refused', message: 'Too old.', mentions: 500 })] })
    const items = [...document.querySelectorAll('#list li')]
    expect(items[0]!.className).toBe('unread')
    expect(items[0]!.querySelector('.badge')).toBeNull()
    expect(items[1]!.querySelector('.badge')!.textContent).toBe('5')
    expect(items[2]!.querySelector('.badge')!.textContent).toBe('!')
    expect(tiles()[1]!.getAttribute('aria-label')).toBe('b.example, current, 5 mentions')
    expect(tiles()[0]!.getAttribute('aria-label')).toBe('a.example, unread')
    expect(tiles()[2]!.title).toBe('c.example: Too old.')
    await open({ ...three, instances: [server('a', { mentions: 150 })] })
    expect(document.querySelector('.badge')!.textContent).toBe('99+')
    await open({ ...three, instances: [server('a', { mentions: 1 }), server('b', { status: 'starting' })] })
    expect(tiles()[0]!.getAttribute('aria-label')).toContain('1 mention')
    expect(tiles()[1]!.getAttribute('aria-label')).toContain('connecting')
  })

  it('shows names as text, never as markup', async () => {
    await open({ ...three, instances: [server('a', { name: '<img src=x onerror=alert(1)>', initial: '<' })] })
    expect(document.querySelector('img')).toBeNull()
    expect(tiles()[0]!.getAttribute('aria-label')).toContain('<img')
  })

  it('updates when the host pushes a new state, and ignores one that is not a state', async () => {
    await open(three)
    const rail = (window as unknown as { mnemaRail: { set: (s: unknown) => void } }).mnemaRail
    rail.set({ ...three, instances: [server('x', { mentions: 2 })], active: 'x' })
    expect(tiles().map((t) => t.textContent)).toEqual(['X'])
    rail.set(null)
    rail.set({ instances: 'nope' })
    expect(tiles()).toHaveLength(1)
  })

  it('switches to the server that was pressed', async () => {
    await open(three)
    tiles()[2]!.click()
    expect(calls('rail_activate')).toEqual([{ id: 'c' }])
  })

  it('opens and closes the page for adding a server', async () => {
    await open(three)
    expect(add().textContent).toBe('+')
    expect(add().getAttribute('aria-label')).toBe('Add server')
    add().click()
    expect(calls('rail_add')).toHaveLength(1)
    await open({ ...three, adding: true })
    expect(add().textContent).toBe('×')
    expect(add().getAttribute('aria-pressed')).toBe('true')
    expect(tiles().map((t) => t.getAttribute('aria-current'))).toEqual(['false', 'false', 'false'])
    add().click()
    expect(calls('rail_cancel_add')).toHaveLength(1)
  })

  it('has nothing to close when no server exists yet, and says when no more fit', async () => {
    await open({ active: null, adding: true, can_add: true, instances: [] })
    expect(add().textContent).toBe('+')
    expect(add().disabled).toBe(false)
    await open({ ...three, can_add: false })
    expect(add().disabled).toBe(true)
    expect(add().title).toBe('No further server can be added.')
    expect(add().getAttribute('aria-label')).toContain('No further server')
  })

  it('moves focus with the arrow keys and reorders with Alt', async () => {
    await open(three)
    tiles()[0]!.focus()
    key(tiles()[0]!, 'ArrowDown')
    expect(document.activeElement).toBe(tiles()[1])
    key(tiles()[1]!, 'ArrowUp')
    expect(document.activeElement).toBe(tiles()[0])
    key(tiles()[0]!, 'ArrowUp')
    expect(document.activeElement).toBe(tiles()[0])
    key(tiles()[1]!, 'ArrowDown', { altKey: true })
    expect(calls('rail_move')).toEqual([{ id: 'b', to: 2 }])
    key(tiles()[2]!, 'ArrowDown', { altKey: true })
    key(tiles()[0]!, 'ArrowUp', { altKey: true })
    expect(calls('rail_move')).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(0)
    expect(document.getElementById('live')!.textContent).toBe('b.example moved to position 3')
  })

  it('reorders by dragging a tile onto another', async () => {
    await open(three)
    const drag = (type: string, el: Element) => el.dispatchEvent(Object.assign(new Event(type, { bubbles: true, cancelable: true }), { dataTransfer: { effectAllowed: '', setData: vi.fn() } }))
    drag('dragstart', tiles()[0]!)
    drag('dragover', tiles()[2]!)
    expect(tiles()[2]!.classList.contains('over')).toBe(true)
    drag('drop', tiles()[2]!)
    expect(calls('rail_move')).toEqual([{ id: 'a', to: 2 }])
    drag('dragend', tiles()[0]!)
    expect(document.querySelector('.over')).toBeNull()
    // Dropping a tile on itself, or with nothing dragged, does nothing.
    drag('drop', tiles()[1]!)
    drag('dragstart', tiles()[1]!)
    drag('drop', tiles()[1]!)
    expect(calls('rail_move')).toHaveLength(1)
  })

  it('offers move and remove in a menu, and reconnect only for a server that is unavailable', async () => {
    await open({ ...three, instances: [server('a'), server('b', { status: 'refused', message: 'x' })] })
    tiles()[0]!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    expect(document.getElementById('menu')!.hidden).toBe(false)
    expect(menuButtons().map((b) => b.textContent)).toEqual(['Move up', 'Move down', 'Remove…'])
    expect(menuButtons()[0]!.disabled).toBe(true)
    menuButtons()[1]!.click()
    expect(calls('rail_move')).toEqual([{ id: 'a', to: 1 }])
    expect(document.getElementById('menu')!.hidden).toBe(true)
    tiles()[1]!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
    expect(menuButtons().map((b) => b.textContent)).toEqual(['Reconnect', 'Move up', 'Move down', 'Remove…'])
    expect(menuButtons()[2]!.disabled).toBe(true)
    menuButtons()[0]!.click()
    expect(calls('rail_reconnect')).toEqual([{ id: 'b' }])
  })

  it('asks before it removes, and says what removing means', async () => {
    await open(three)
    key(tiles()[0]!, 'Delete')
    expect(document.querySelector('#menu .note')!.textContent).toContain('all local data')
    expect(calls('rail_remove')).toHaveLength(0)
    menuButtons().find((b) => b.textContent === 'Cancel')!.click()
    expect(calls('rail_remove')).toHaveLength(0)
    key(tiles()[0]!, 'ContextMenu')
    menuButtons().find((b) => b.textContent === 'Remove…')!.click()
    menuButtons().find((b) => b.textContent === 'Remove server')!.click()
    expect(calls('rail_remove')).toEqual([{ id: 'a' }])
    await vi.advanceTimersByTimeAsync(0)
    expect(document.getElementById('live')!.textContent).toBe('a.example removed')
  })

  it('closes the menu with Escape, with a press elsewhere and when the window loses focus', async () => {
    await open(three)
    const opened = () => { key(tiles()[0]!, 'F10', { shiftKey: true }); return document.getElementById('menu')!.hidden }
    expect(opened()).toBe(false)
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    expect(document.getElementById('menu')!.hidden).toBe(true)
    opened()
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }))
    expect(document.getElementById('menu')!.hidden).toBe(true)
    opened()
    window.dispatchEvent(new Event('blur'))
    expect(document.getElementById('menu')!.hidden).toBe(true)
    opened()
    document.getElementById('menu')!.dispatchEvent(new Event('pointerdown', { bubbles: true }))
    expect(document.getElementById('menu')!.hidden).toBe(false)
  })

  it('speaks German on a German system', async () => {
    await open({ ...three, active: 'a', instances: [server('a', { mentions: 2 })] }, 'de-DE')
    expect(document.getElementById('list')!.getAttribute('aria-label')).toBe('Server')
    expect(tiles()[0]!.getAttribute('aria-label')).toBe('a.example, aktiv, 2 Erwähnungen')
    expect(add().title).toBe('Server hinzufügen')
  })

  it('uses the bridge of the host that provides one instead of Tauri', async () => {
    const own = vi.fn(async (cmd: string) => (cmd === 'rail_state' ? three : undefined))
    Object.defineProperty(window, '__MNEMA_RAIL_HOST__', { value: { invoke: own }, configurable: true })
    await open(three)
    tiles()[0]!.click()
    expect(own).toHaveBeenCalledWith('rail_activate', { id: 'a' })
    expect(invoke).not.toHaveBeenCalled()
  })

  it('still draws when the host does not answer', async () => {
    state = three
    Object.defineProperty(navigator, 'language', { value: 'en-US', configurable: true })
    document.documentElement.innerHTML = html.replace(/^[\s\S]*<html[^>]*>/, '').replace(/<\/html>\s*$/, '').replace(/<(link|script)[^>]*>(<\/script>)?/g, '')
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke: vi.fn(async () => { throw new Error('gone') }) }, configurable: true })
    // eslint-disable-next-line no-new-func
    new Function(script)()
    await vi.advanceTimersByTimeAsync(0)
    expect(tiles()).toHaveLength(0)
    expect(add().textContent).toBe('+')
  })
})
