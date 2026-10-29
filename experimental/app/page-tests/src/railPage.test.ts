// The server rail (experimental/app/rail-ui), run against a fake host. It is
// plain script served by the executable (and by the Linux client, unchanged),
// so it is tested here.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Vitest runs from the page-tests directory.
const dir = resolve(process.cwd(), '../rail-ui')
const html = readFileSync(resolve(dir, 'index.html'), 'utf8')
const script = readFileSync(resolve(dir, 'rail.js'), 'utf8')

interface Server { id: string, name: string, original: string, alias: string | null, initial: string, hue: number, status: string, message: string | null, unread: number, mentions: number, icon: string | null }
type Item = { kind: 'server', id: string } | { kind: 'folder', id: string, name: string, color: string | null, collapsed: boolean, servers: string[] }
interface State { active: string | null, adding: boolean, can_add: boolean, instances: Server[], items?: Item[] }

const server = (id: string, extra: Partial<Server> = {}): Server => ({ id, name: `${id}.example`, original: `${id}.example`, alias: null, initial: id[0]!.toUpperCase(), hue: 120, status: 'ready', message: null, unread: 0, mentions: 0, icon: null, ...extra })
const folder = (id: string, servers: string[], extra: Partial<Extract<Item, { kind: 'folder' }>> = {}): Item => ({ kind: 'folder', id, name: '', color: null, collapsed: false, servers, ...extra })
const only = (id: string): Item => ({ kind: 'server', id })
const three: State = { active: 'b', adding: false, can_add: true, instances: [server('a'), server('b'), server('c')] }

let invoke: ReturnType<typeof vi.fn>
let state: State

// Parse the page into an inert template and drop the stylesheet and script elements
// the executable loads itself; they are not fetched in the test window.
function pageMarkup(): string {
  const template = document.createElement('template')
  template.innerHTML = html
  template.content.querySelectorAll('link, script').forEach((el) => el.remove())
  return template.innerHTML
}

async function open(initial: State, lang = 'en-US', replies: Record<string, unknown> = {}) {
  state = initial
  Object.defineProperty(navigator, 'language', { value: lang, configurable: true })
  document.documentElement.innerHTML = pageMarkup()
  invoke = vi.fn(async (cmd: string) => (cmd === 'rail_state' ? state : replies[cmd]))
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke }, configurable: true })
  // The page script is plain browser code; run it in the test window.
  // eslint-disable-next-line no-new-func
  new Function(script)()
  await vi.advanceTimersByTimeAsync(0)
}

const tiles = () => [...document.querySelectorAll<HTMLButtonElement>('.tile')]
const add = () => document.getElementById('add') as HTMLButtonElement
const key = (el: Element, k: string, init: KeyboardEventInit = {}) => el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...init }))
const calls = (cmd: string) => invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args)
const rail = () => (window as unknown as { mnemaRail: { set: (s: unknown) => void, chosen: (token: string, id: string) => void } }).mnemaRail
const live = () => document.getElementById('live')!.textContent
const folderButtons = () => [...document.querySelectorAll<HTMLButtonElement>('.folder-btn')]

interface MenuItem { id?: string, text?: string, disabled?: boolean, checked?: boolean, separator?: boolean, items?: MenuItem[] }
interface Spec { token: string, items: MenuItem[] }
/** The menu the page last asked the host to show, and the position it asked for. */
function shown(): { spec: Spec, x: number, y: number } {
  const all = calls('rail_menu') as { spec: string, x: number, y: number }[]
  const last = all[all.length - 1]!
  return { spec: JSON.parse(last.spec) as Spec, x: last.x, y: last.y }
}
const texts = (items: MenuItem[]) => items.filter((i) => !i.separator).map((i) => i.text)
const pick = (id: string) => {
  const { spec } = shown()
  rail().chosen(spec.token, id)
}
const rightClick = (el: Element) => el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
const flat = (items: MenuItem[]): MenuItem[] => items.flatMap((i) => [i, ...(i.items ? flat(i.items) : [])])

/** A host that answers the commands in `answers`; anything else gets the state or nothing. */
function answer(answers: Record<string, unknown>) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd in answers) {
      const value = answers[cmd]
      return typeof value === 'function' ? (value as () => unknown)() : value
    }
    return cmd === 'rail_state' ? state : undefined
  })
}

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

  it('moves focus with the arrow keys and moves a server one step with Alt', async () => {
    await open(three)
    tiles()[0]!.focus()
    key(tiles()[0]!, 'ArrowDown')
    expect(document.activeElement).toBe(tiles()[1])
    key(tiles()[1]!, 'ArrowUp')
    expect(document.activeElement).toBe(tiles()[0])
    key(tiles()[0]!, 'ArrowUp')
    expect(document.activeElement).toBe(tiles()[0])
    key(tiles()[1]!, 'ArrowDown', { altKey: true })
    expect(calls('rail_edit')).toEqual([{ op: 'step', id: 'b', delta: 1 }])
    await vi.advanceTimersByTimeAsync(0)
    expect(live()).toBe('b.example moved to position 2')
  })

  it('does not announce a change the host refused', async () => {
    await open(three)
    answer({ rail_edit: () => { throw new Error('denied') } })
    key(tiles()[1]!, 'ArrowDown', { altKey: true })
    await vi.advanceTimersByTimeAsync(0)
    expect(live()).toBe('The change could not be saved')
  })

  describe('menu', () => {
    it('is shown by the host, above everything, and never drawn by the page', async () => {
      await open(three)
      expect(document.getElementById('menu')).toBeNull()
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_menu')).toHaveLength(1)
      // The pointer places it; the page sends no position of its own.
      expect(shown()).toMatchObject({ x: -1, y: -1 })
      expect(document.querySelector('[role="menu"], [role="menuitem"]')).toBeNull()
    })

    it('opens beside the tile for the keyboard', async () => {
      await open(three)
      tiles()[0]!.getBoundingClientRect = () => ({ left: 12, right: 56, top: 40, bottom: 84, width: 44, height: 44, x: 12, y: 40, toJSON: () => ({}) })
      key(tiles()[0]!, 'ContextMenu')
      await vi.advanceTimersByTimeAsync(0)
      expect(shown()).toMatchObject({ x: 62, y: 40 })
      key(tiles()[1]!, 'F10', { shiftKey: true })
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_menu')).toHaveLength(2)
    })

    it('offers rename, folders, move and remove, and reconnect only for a server that is unavailable', async () => {
      await open({ ...three, instances: [server('a'), server('b', { status: 'refused', message: 'x' })] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).toEqual(['a.example', 'Rename…', 'Add to folder', 'Move up', 'Move down', 'Remove…'])
      expect(shown().spec.items.find((i) => i.id === 'up')!.disabled).toBe(true)
      expect(shown().spec.items.find((i) => i.id === 'down')!.disabled).toBeFalsy()
      pick('down')
      expect(calls('rail_edit')).toEqual([{ op: 'step', id: 'a', delta: 1 }])
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).toContain('Reconnect')
      expect(shown().spec.items.find((i) => i.id === 'down')!.disabled).toBe(true)
      pick('reconnect')
      expect(calls('rail_reconnect')).toEqual([{ id: 'b' }])
    })

    it('only does what the latest menu offers, once', async () => {
      await open(three)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      const first = shown().spec
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      rail().chosen(first.token, 'down')
      expect(calls('rail_edit')).toHaveLength(0)
      pick('nothing-like-it')
      expect(calls('rail_edit')).toHaveLength(0)
      pick('up')
      pick('up')
      expect(calls('rail_edit')).toEqual([{ op: 'step', id: 'b', delta: -1 }])
    })

    it('keeps the host-facing text clean: no control characters, bounded, never empty', async () => {
      await open({ ...three, instances: [server('a', { name: 'Bad\u0007\nName', alias: 'x'.repeat(200) }), server('b', { name: '\u0001' })] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      for (const item of flat(shown().spec.items)) if (item.text !== undefined) expect(item.text).toMatch(/^[^\u0000-\u001f\u007f-\u009f]{1,100}$/)
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(shown().spec.items[0]!.text).toBe('–')
    })

    it('uses ids and text the host accepts, with unique ids', async () => {
      await open({ ...three, items: [folder('f-1', ['a']), only('b'), only('c')] })
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      const ids = flat(shown().spec.items).filter((i) => i.id).map((i) => i.id!)
      expect(new Set(ids).size).toBe(ids.length)
      for (const id of ids) expect(id).toMatch(/^[a-z0-9:_.-]{1,48}$/)
      expect(shown().spec.token).toMatch(/^[a-z0-9-]{1,40}$/)
      expect(shown().spec.items.length).toBeLessThanOrEqual(24)
    })
  })

  describe('notification level of a server', () => {
    const info = (value: unknown, ...set: unknown[]) => answer({ rail_notify_get: value, rail_notify_set: set.length ? set[0] : null })
    const levels = () => shown().spec.items.find((i) => i.id === 'notifications')?.items

    it('offers all, mentions only and mute with the current level checked', async () => {
      await open(three)
      info({ supported: true, level: 'mentions', suppress_everyone: false })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_notify_get')).toEqual([{ id: 'a' }])
      expect(texts(shown().spec.items)).toContain('Notifications')
      expect(levels()!.map((i) => i.text)).toEqual(['All messages', 'Only mentions', 'Mute'])
      expect(levels()!.map((i) => i.checked)).toEqual([false, true, false])
    })

    it('sets the chosen level on that server and announces it', async () => {
      await open(three)
      info({ supported: true, level: 'all', suppress_everyone: true })
      rightClick(tiles()[2]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('notify-mute')
      expect(calls('rail_notify_set')).toEqual([{ id: 'c', level: 'mute' }])
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('c.example: Mute')
    })

    it('says so when the server refuses the change', async () => {
      await open(three)
      info({ supported: true, level: 'all', suppress_everyone: false }, undefined)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('notify-mentions')
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('a.example: the setting could not be changed')
    })

    it('leaves the entries out for an older server, an unknown answer and a server that is not ready', async () => {
      await open({ ...three, instances: [server('a'), server('b'), server('c', { status: 'refused', message: 'x' })] })
      for (const value of [{ supported: false, level: 'all' }, undefined, { supported: true, level: 'bogus' }]) {
        info(value)
        rightClick(tiles()[0]!)
        await vi.advanceTimersByTimeAsync(0)
        expect(levels()).toBeUndefined()
      }
      info({ supported: true, level: 'all' })
      rightClick(tiles()[2]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_notify_get').every((a) => (a as { id: string }).id === 'a')).toBe(true)
      expect(levels()).toBeUndefined()
    })

    it('does not keep the menu waiting for a server that is slow to answer', async () => {
      await open(three)
      answer({ rail_notify_get: () => new Promise(() => {}) })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(100)
      expect(calls('rail_menu')).toHaveLength(0)
      await vi.advanceTimersByTimeAsync(300)
      expect(calls('rail_menu')).toHaveLength(1)
      expect(levels()).toBeUndefined()
    })

    it('shows no menu for a server that is gone by the time the answer arrives', async () => {
      await open(three)
      let release: (v: unknown) => void = () => {}
      answer({ rail_notify_get: () => new Promise((r) => { release = r }) })
      rightClick(tiles()[0]!)
      rail().set({ ...three, instances: [server('b'), server('c')] })
      release({ supported: true, level: 'all' })
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_menu')).toHaveLength(0)
    })

    it('answers in German', async () => {
      await open(three, 'de-DE')
      info({ supported: true, level: 'all', suppress_everyone: false })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(levels()!.map((i) => i.text)).toEqual(['Alle Nachrichten', 'Nur Erwähnungen', 'Stummschalten'])
      expect(texts(shown().spec.items)).toEqual(['a.example', 'Benachrichtigungen', 'Umbenennen …', 'Zu Ordner hinzufügen', 'Nach oben', 'Nach unten', 'Entfernen …'])
    })
  })

  describe('removal', () => {
    it('asks before it removes, in a window of its own, and says what removing means', async () => {
      await open(three)
      answer({ rail_dialog: { button: 'cancel', value: null, color: null } })
      key(tiles()[0]!, 'Delete')
      await vi.advanceTimersByTimeAsync(0)
      const spec = JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec)
      expect(spec.message).toContain('all local data')
      expect(spec.buttons.map((b: { id: string }) => b.id)).toEqual(['remove', 'cancel'])
      expect(spec.buttons.find((b: { id: string }) => b.id === 'remove').danger).toBe(true)
      expect(spec.cancel).toBe('cancel')
      expect(calls('rail_remove')).toHaveLength(0)
    })

    it('removes after the question was answered with yes, from the menu too', async () => {
      await open(three)
      answer({ rail_dialog: { button: 'remove', value: null, color: null } })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('remove')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_remove')).toEqual([{ id: 'a' }])
      expect(live()).toBe('a.example removed')
    })

    it('does not remove when the window was closed or the host refused it', async () => {
      await open(three)
      answer({ rail_dialog: () => { throw new Error('busy') } })
      key(tiles()[0]!, 'Delete')
      await vi.advanceTimersByTimeAsync(0)
      answer({ rail_dialog: { button: 'remove', value: null, color: null }, rail_remove: () => { throw new Error('denied') } })
      key(tiles()[0]!, 'Delete')
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('a.example: the server could not be removed')
    })
  })

  describe('names', () => {
    it('shows the local name and keeps the original in the tooltip and the label', async () => {
      await open({ ...three, instances: [server('a', { name: 'Lantern', alias: 'Lantern', original: 'a.example' }), server('b')] })
      expect(tiles()[0]!.title).toBe('Lantern (a.example)')
      expect(tiles()[0]!.getAttribute('aria-label')).toBe('Lantern (a.example)')
      expect(tiles()[1]!.title).toBe('b.example')
    })

    it('asks for the new name, with the current local one and the original as a hint', async () => {
      await open({ ...three, instances: [server('a', { name: 'Lantern', alias: 'Lantern', original: 'a.example' })] })
      answer({ rail_dialog: { button: 'cancel', value: null, color: null } })
      key(tiles()[0]!, 'F2')
      await vi.advanceTimersByTimeAsync(0)
      const spec = JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec)
      expect(spec.input).toMatchObject({ value: 'Lantern', max: 40, placeholder: 'a.example' })
      expect(spec.message).toContain('only applies on this device')
      expect(spec.message).toContain('a.example')
      expect(calls('rail_edit')).toHaveLength(0)
    })

    it('saves the name the person typed', async () => {
      await open(three)
      answer({ rail_dialog: { button: 'save', value: '  Home base ', color: null } })
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('rename')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toEqual([{ op: 'rename', id: 'b', name: 'Home base' }])
      expect(live()).toBe('Renamed to Home base')
    })

    it('clears the local name when it is emptied or set back to the original', async () => {
      await open({ ...three, instances: [server('a', { name: 'Lantern', alias: 'Lantern' })] })
      answer({ rail_dialog: { button: 'save', value: '   ', color: null } })
      key(tiles()[0]!, 'F2')
      await vi.advanceTimersByTimeAsync(0)
      answer({ rail_dialog: { button: 'save', value: 'a.example', color: null } })
      key(tiles()[0]!, 'F2')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toEqual([{ op: 'rename', id: 'a', name: '' }, { op: 'rename', id: 'a', name: '' }])
      expect(live()).toBe('a.example: using the name of the server')
    })

    it('offers to use the name of the server again only while there is a local one', async () => {
      await open({ ...three, instances: [server('a', { name: 'Lantern', alias: 'Lantern' }), server('b')] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).toContain('Use the name of the server')
      pick('unalias')
      expect(calls('rail_edit')).toEqual([{ op: 'rename', id: 'a', name: '' }])
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).not.toContain('Use the name of the server')
    })
  })

  describe('folders', () => {
    const grouped: State = { ...three, active: 'b', instances: [server('a'), server('b'), server('c', { mentions: 3 }), server('d')], items: [folder('f-1', ['a', 'b'], { name: 'Work', color: '#2da771' }), folder('f-2', ['c', 'd'], { collapsed: true }), ] }

    it('draws an open folder around its servers and a closed one as a grid of its first servers', async () => {
      await open(grouped)
      expect(tiles().map((t) => t.dataset.id)).toEqual(['a', 'b'])
      expect(folderButtons().map((b) => b.getAttribute('aria-expanded'))).toEqual(['true', 'false'])
      expect(folderButtons()[0]!.getAttribute('aria-label')).toBe('Work, 2 servers, expanded')
      expect(document.querySelector('.folder-list')!.getAttribute('aria-label')).toBe('Work')
      expect((document.querySelector('.folder') as HTMLElement).style.getPropertyValue('--folder')).toBe('#2da771')
      const closed = document.querySelectorAll('.folder')[1]!
      expect([...closed.querySelectorAll('.mini')].map((m) => m.textContent)).toEqual(['C', 'D'])
      expect(folderButtons()[1]!.title).toBe('Folder: c.example, d.example')
    })

    it('adds up what is inside a closed folder: mentions, unread, and where the front server is', async () => {
      await open({ ...grouped, active: 'd' })
      const closed = document.querySelectorAll('.folder')[1]!
      expect(closed.querySelector('.badge')!.textContent).toBe('3')
      expect(closed.className).toContain('current')
      expect(folderButtons()[1]!.getAttribute('aria-label')).toBe('Folder, 2 servers, collapsed, 3 mentions')
      await open({ ...grouped, active: 'a', instances: [server('a'), server('b'), server('c', { unread: 1 }), server('d')] })
      expect(document.querySelectorAll('.folder')[1]!.className).toContain('unread')
      expect(document.querySelectorAll('.folder')[1]!.querySelector('.badge')).toBeNull()
      // An open folder leaves the badges to its servers.
      expect(document.querySelectorAll('.folder')[0]!.querySelector('.folder-btn + .badge')).toBeNull()
    })

    it('shows at most four servers in the grid', async () => {
      const five = ['a', 'b', 'c', 'd', 'e']
      await open({ ...three, instances: five.map((id) => server(id)), items: [folder('f-1', five, { collapsed: true })] })
      expect(document.querySelectorAll('.mini')).toHaveLength(4)
      expect(folderButtons()[0]!.getAttribute('aria-label')).toContain('5 servers')
    })

    it('opens and closes on a press and keeps focus on the folder', async () => {
      await open(grouped)
      folderButtons()[1]!.click()
      expect(calls('rail_edit')).toEqual([{ op: 'toggle', id: 'f-2', flag: false }])
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('Folder, expanded')
      folderButtons()[0]!.click()
      expect(calls('rail_edit')[1]).toEqual({ op: 'toggle', id: 'f-1', flag: true })
    })

    it('opens with the right arrow and closes with the left one', async () => {
      await open(grouped)
      key(folderButtons()[1]!, 'ArrowRight')
      key(folderButtons()[1]!, 'ArrowLeft')
      expect(calls('rail_edit')).toEqual([{ op: 'toggle', id: 'f-2', flag: false }])
      key(folderButtons()[0]!, 'ArrowLeft')
      key(folderButtons()[0]!, 'ArrowRight')
      expect(calls('rail_edit')[1]).toEqual({ op: 'toggle', id: 'f-1', flag: true })
      expect(calls('rail_edit')).toHaveLength(2)
    })

    it('moves focus through folders and their servers, and moves a folder as a whole with Alt', async () => {
      await open(grouped)
      const order = () => [...document.querySelectorAll<HTMLElement>('.tile, .folder-btn')].map((b) => b.dataset.id)
      expect(order()).toEqual(['f-1', 'a', 'b', 'f-2'])
      folderButtons()[0]!.focus()
      key(folderButtons()[0]!, 'ArrowDown')
      expect((document.activeElement as HTMLElement).dataset.id).toBe('a')
      key(folderButtons()[1]!, 'ArrowDown', { altKey: true })
      key(folderButtons()[1]!, 'ArrowUp', { altKey: true })
      expect(calls('rail_edit')).toEqual([{ op: 'step', id: 'f-2', delta: 1 }, { op: 'step', id: 'f-2', delta: -1 }])
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('Folder moved')
    })

    it('has a menu for a folder: open, edit, move, delete', async () => {
      await open(grouped)
      rightClick(folderButtons()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).toEqual(['Work', 'Collapse', 'Edit folder…', 'Move up', 'Move down', 'Delete folder'])
      expect(shown().spec.items.find((i) => i.id === 'up')!.disabled).toBe(true)
      pick('toggle')
      expect(calls('rail_edit')).toEqual([{ op: 'toggle', id: 'f-1', flag: true }])
      rightClick(folderButtons()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)[1]).toBe('Expand')
      expect(shown().spec.items.find((i) => i.id === 'down')!.disabled).toBe(true)
      pick('delete')
      expect(calls('rail_edit')[1]).toEqual({ op: 'folder_delete', id: 'f-2' })
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('Folder deleted')
    })

    it('deletes a folder with the Delete key without asking: its servers stay', async () => {
      await open(grouped)
      key(folderButtons()[0]!, 'Delete')
      expect(calls('rail_edit')).toEqual([{ op: 'folder_delete', id: 'f-1' }])
      expect(calls('rail_dialog')).toHaveLength(0)
    })

    it('edits name and colour in a question of its own', async () => {
      await open(grouped)
      answer({ rail_dialog: { button: 'save', value: 'Friends', color: '#3b82f6' } })
      key(folderButtons()[0]!, 'F2')
      await vi.advanceTimersByTimeAsync(0)
      const spec = JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec)
      expect(spec.input).toMatchObject({ value: 'Work', max: 40 })
      expect(spec.colors.value).toBe('#2da771')
      expect(spec.colors.choices[0]).toEqual({ value: '', label: 'None' })
      expect(spec.colors.choices.every((c: { value: string }) => c.value === '' || /^#[0-9a-f]{6}$/.test(c.value))).toBe(true)
      expect(calls('rail_edit')).toEqual([{ op: 'folder_edit', id: 'f-1', name: 'Friends', color: '#3b82f6' }])
    })

    it('does not touch the folder when the question is cancelled', async () => {
      await open(grouped)
      answer({ rail_dialog: { button: 'cancel', value: 'x', color: '#3b82f6' } })
      key(folderButtons()[0]!, 'F2')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toHaveLength(0)
    })

    it('lists the folders of the rail under Add to folder, and the one the server is in is not offered again', async () => {
      await open(grouped)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      const into = shown().spec.items.find((i) => i.id === 'folders')!.items!
      expect(into.map((i) => i.text)).toEqual(['New folder', 'Work', 'Folder'])
      expect(into.map((i) => !!i.disabled)).toEqual([false, true, false])
      expect(texts(shown().spec.items)).toContain('Remove from folder')
      pick('to-f-2')
      expect(calls('rail_edit')).toEqual([{ op: 'place', id: 'a', target: 'f-2', before: '' }])
      pick('ungroup')
      expect(calls('rail_edit')).toHaveLength(1)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('ungroup')
      expect(calls('rail_edit')[1]).toEqual({ op: 'ungroup', id: 'a' })
    })

    it('can move a server out of the top of its folder by stepping up', async () => {
      await open(grouped)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(shown().spec.items.find((i) => i.id === 'up')!.disabled).toBeFalsy()
    })

    it('makes a folder around a server from the menu and asks for its name', async () => {
      await open(three)
      const made: State = { ...three, items: [only('a'), folder('f-9', ['b']), only('c')] }
      answer({ rail_state: () => made, rail_dialog: { button: 'save', value: 'Mine', color: '' } })
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('folder-new')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')[0]).toEqual({ op: 'new_folder', id: 'b' })
      expect(calls('rail_dialog')).toHaveLength(1)
      expect(calls('rail_edit')[1]).toEqual({ op: 'folder_edit', id: 'f-9', name: 'Mine', color: '' })
    })

    it('draws a state without folders (an older host) as a flat list', async () => {
      await open({ ...three, items: undefined })
      expect(tiles()).toHaveLength(3)
      expect(folderButtons()).toHaveLength(0)
    })

    it('shows folder names as text', async () => {
      await open({ ...three, items: [folder('f-1', ['a'], { name: '<img src=x onerror=alert(1)>' }), only('b'), only('c')] })
      expect(document.querySelector('.folder img')).toBeNull()
      expect(folderButtons()[0]!.getAttribute('aria-label')).toContain('<img')
    })
  })

  describe('icons', () => {
    const png = 'data:image/png;base64,iVBORw0KGgo='
    const withIcon = (extra: Partial<Server> = {}): State => ({ ...three, instances: [server('a', { icon: 'aaaa', ...extra }), server('b')] })

    it('shows the initial until the icon the host holds for a server arrives', async () => {
      await open(withIcon(), 'en-US', { rail_icon: new Promise(() => {}) })
      expect(tiles()[0]!.textContent).toBe('A')
      expect(calls('rail_icon')).toEqual([{ id: 'a' }])
    })

    it('puts the image in the tile when it arrives, and fetches a version only once', async () => {
      await open(withIcon(), 'en-US', { rail_icon: png })
      const img = tiles()[0]!.querySelector('img')!
      expect(img.getAttribute('src')).toBe(png)
      expect(img.getAttribute('alt')).toBe('')
      expect(tiles()[1]!.querySelector('img')).toBeNull()
      rail().set(withIcon({ unread: 1 }))
      await vi.advanceTimersByTimeAsync(0)
      expect(tiles()[0]!.querySelector('img')).not.toBeNull()
      expect(calls('rail_icon')).toHaveLength(1)
    })

    it('asks again when the version changes, and falls back to the initial when the icon is gone', async () => {
      await open(withIcon(), 'en-US', { rail_icon: png })
      rail().set(withIcon({ icon: 'bbbb' }))
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_icon')).toHaveLength(2)
      rail().set(withIcon({ icon: null }))
      expect(tiles()[0]!.querySelector('img')).toBeNull()
      expect(tiles()[0]!.textContent).toBe('A')
    })

    it('shows only images, never what else the host might send', async () => {
      for (const reply of ['https://elsewhere.example/icon.png', 'javascript:alert(1)', 42, null]) {
        await open(withIcon(), 'en-US', { rail_icon: reply })
        expect(document.querySelector('img')).toBeNull()
        expect(tiles()[0]!.textContent).toBe('A')
      }
    })

    it('uses the icon in the grid of a closed folder too', async () => {
      await open({ ...withIcon(), items: [folder('f-1', ['a', 'b'], { collapsed: true })] }, 'en-US', { rail_icon: png })
      const minis = [...document.querySelectorAll('.mini')]
      expect(minis[0]!.querySelector('img')!.getAttribute('src')).toBe(png)
      expect(minis[1]!.textContent).toBe('B')
    })
  })

  describe('drag and drop', () => {
    const drag = (type: string, el: Element, y = 22) => {
      const event = Object.assign(new Event(type, { bubbles: true, cancelable: true }), { dataTransfer: { effectAllowed: '', setData: vi.fn() }, clientY: y })
      el.dispatchEvent(event)
      return event
    }
    // Every tile is 44 px tall, one under the other; y is the pointer in the tile.
    const lay = () => {
      for (const [i, el] of [...document.querySelectorAll<HTMLElement>('.tile, .folder-btn')].entries()) {
        el.getBoundingClientRect = () => ({ left: 0, right: 44, top: i * 50, bottom: i * 50 + 44, width: 44, height: 44, x: 0, y: i * 50, toJSON: () => ({}) })
      }
    }
    const at = (i: number, within: number) => i * 50 + within
    const grouped: State = { ...three, active: 'b', instances: [server('a'), server('b'), server('c'), server('d')], items: [only('a'), folder('f-1', ['b', 'c']), only('d')] }

    it('puts a server before or after another when dropped on its upper or lower edge', async () => {
      await open(three)
      lay()
      drag('dragstart', tiles()[0]!)
      drag('dragover', tiles()[2]!, at(2, 40))
      expect(tiles()[2]!.closest('li')!.classList.contains('drop-after')).toBe(true)
      drag('drop', tiles()[2]!, at(2, 40))
      expect(calls('rail_edit')).toEqual([{ op: 'place', id: 'a', target: '', before: '' }])
      drag('dragend', tiles()[0]!)
      expect(document.querySelector('.drop-after, .drop-before, .drop-into')).toBeNull()
      drag('dragstart', tiles()[2]!)
      drag('drop', tiles()[0]!, at(0, 3))
      expect(calls('rail_edit')[1]).toEqual({ op: 'place', id: 'c', target: '', before: 'a' })
    })

    it('drops after the one in the middle of the rail before the one that follows', async () => {
      await open(three)
      lay()
      drag('dragstart', tiles()[2]!)
      drag('drop', tiles()[0]!, at(0, 40))
      expect(calls('rail_edit')).toEqual([{ op: 'place', id: 'c', target: '', before: 'b' }])
    })

    it('makes a folder when a server is dropped onto the middle of another', async () => {
      await open(three)
      lay()
      drag('dragstart', tiles()[0]!)
      drag('dragover', tiles()[1]!, at(1, 22))
      expect(tiles()[1]!.classList.contains('drop-into')).toBe(true)
      drag('drop', tiles()[1]!, at(1, 22))
      expect(calls('rail_edit')).toEqual([{ op: 'group', id: 'a', target: 'b' }])
    })

    it('does nothing when a tile is dropped on itself, or when nothing is dragged', async () => {
      await open(three)
      lay()
      drag('drop', tiles()[1]!)
      drag('dragstart', tiles()[1]!)
      drag('dragover', tiles()[1]!)
      expect(document.querySelector('.drop-into')).toBeNull()
      drag('drop', tiles()[1]!)
      expect(calls('rail_edit')).toHaveLength(0)
    })

    it('moves a server into a folder when dropped on the middle of the folder, or next to a member', async () => {
      await open(grouped)
      lay()
      const order = [...document.querySelectorAll<HTMLElement>('.tile, .folder-btn')].map((b) => b.dataset.id)
      expect(order).toEqual(['a', 'f-1', 'b', 'c', 'd'])
      drag('dragstart', document.querySelector('[data-id="d"]')!)
      drag('drop', folderButtons()[0]!, at(1, 22))
      expect(calls('rail_edit')[0]).toEqual({ op: 'place', id: 'd', target: 'f-1', before: '' })
      drag('dragstart', document.querySelector('[data-id="d"]')!)
      drag('drop', document.querySelector('[data-id="b"]')!, at(2, 3))
      expect(calls('rail_edit')[1]).toEqual({ op: 'place', id: 'd', target: 'f-1', before: 'b' })
      drag('dragstart', document.querySelector('[data-id="a"]')!)
      drag('drop', document.querySelector('[data-id="c"]')!, at(3, 40))
      expect(calls('rail_edit')[2]).toEqual({ op: 'place', id: 'a', target: 'f-1', before: '' })
    })

    it('places a server around a folder when dropped on its edges', async () => {
      await open(grouped)
      lay()
      drag('dragstart', document.querySelector('[data-id="d"]')!)
      drag('drop', folderButtons()[0]!, at(1, 2))
      expect(calls('rail_edit')[0]).toEqual({ op: 'place', id: 'd', target: '', before: 'f-1' })
      drag('dragstart', document.querySelector('[data-id="a"]')!)
      drag('drop', folderButtons()[0]!, at(1, 42))
      expect(calls('rail_edit')[1]).toEqual({ op: 'place', id: 'a', target: '', before: 'd' })
    })

    it('moves a folder between items and never into another folder', async () => {
      await open(grouped)
      lay()
      drag('dragstart', folderButtons()[0]!)
      drag('dragover', folderButtons()[0]!)
      drag('drop', document.querySelector('[data-id="a"]')!, at(0, 3))
      expect(calls('rail_edit')[0]).toEqual({ op: 'place', id: 'f-1', target: '', before: 'a' })
      drag('dragstart', folderButtons()[0]!)
      // Onto the middle of the last server: still a place between items, not a group.
      drag('drop', document.querySelector('[data-id="d"]')!, at(4, 22))
      expect(calls('rail_edit')[1]).toEqual({ op: 'place', id: 'f-1', target: '', before: '' })
      // Onto a member of its own folder: nowhere to go.
      drag('dragstart', folderButtons()[0]!)
      drag('drop', document.querySelector('[data-id="b"]')!, at(2, 3))
      expect(calls('rail_edit')).toHaveLength(2)
    })

    it('drops below the last item to the end of the rail', async () => {
      await open(grouped)
      lay()
      drag('dragstart', document.querySelector('[data-id="b"]')!)
      const below = drag('dragover', document.getElementById('rail')!)
      expect(below.defaultPrevented).toBe(true)
      drag('drop', document.getElementById('rail')!)
      expect(calls('rail_edit')).toEqual([{ op: 'place', id: 'b', target: '', before: '' }])
    })

    it('holds a push of the host until the drag is over', async () => {
      await open(three)
      lay()
      drag('dragstart', tiles()[0]!)
      const dragged = tiles()[0]!
      rail().set({ ...three, instances: [server('a', { mentions: 4 }), server('b'), server('c')] })
      expect(tiles()[0]).toBe(dragged)
      drag('dragend', dragged)
      expect(tiles()[0]).not.toBe(dragged)
      expect(tiles()[0]!.getAttribute('aria-label')).toContain('4 mentions')
    })

    it('is over when the pointer is used again, even if the end of the drag was not seen', async () => {
      await open(three)
      lay()
      drag('dragstart', tiles()[0]!)
      rail().set({ ...three, instances: [server('a', { mentions: 4 }), server('b'), server('c')] })
      document.dispatchEvent(new Event('pointerdown'))
      expect(tiles()[0]!.getAttribute('aria-label')).toContain('4 mentions')
    })
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
    document.documentElement.innerHTML = pageMarkup()
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke: vi.fn(async () => { throw new Error('gone') }) }, configurable: true })
    // eslint-disable-next-line no-new-func
    new Function(script)()
    await vi.advanceTimersByTimeAsync(0)
    expect(tiles()).toHaveLength(0)
    expect(add().textContent).toBe('+')
  })
})
