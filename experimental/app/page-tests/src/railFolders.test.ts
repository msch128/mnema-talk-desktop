// The server rail (experimental/app/rail-ui): folders, extras, icons and drag and drop.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { add, answer, calls, folder, folderButtons, invoke, key, live, only, open, pick, rail, rightClick, runPage, server, shown, texts, three, tiles } from './railHarness.ts'
import type { Server, State } from './railHarness.ts'

describe('server rail', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => { vi.useRealTimers(); Reflect.deleteProperty(window, '__MNEMA_RAIL_HOST__') })

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

  describe('extras', () => {
    const later = Math.floor(Date.now() / 1000) + 3600

    it('shows a muted server dimmed with a bell, and counts none of its mentions', async () => {
      await open({ ...three, active: 'a', instances: [server('a'), server('b', { mentions: 4, muted_until: later } as Partial<Server>), server('c', { unread: 2, muted_until: -1 } as Partial<Server>)] })
      const items = [...document.querySelectorAll('#list li')]
      expect(items[1]!.className).toContain('muted')
      expect(items[1]!.querySelector('.mute')).not.toBeNull()
      expect(items[1]!.querySelector('.badge')).toBeNull()
      expect(items[2]!.className).not.toContain('unread')
      expect(tiles()[1]!.getAttribute('aria-label')).toBe('b.example, muted')
    })

    it('adds up only the servers that are not muted in a closed folder', async () => {
      await open({ ...three, instances: [server('a', { mentions: 2 }), server('b', { mentions: 9, muted_until: -1 } as Partial<Server>)], items: [folder('f-1', ['a', 'b'], { collapsed: true })] })
      expect(document.querySelector('.folder .badge')!.textContent).toBe('2')
    })

    it('treats a mute that has run out as none, and redraws when it does', async () => {
      const soon = Math.floor(Date.now() / 1000) + 2
      await open({ ...three, instances: [server('a', { muted_until: soon } as Partial<Server>)] })
      expect(document.querySelector('li.muted')).not.toBeNull()
      await vi.advanceTimersByTimeAsync(3000)
      expect(document.querySelector('li.muted')).toBeNull()
      await open({ ...three, instances: [server('a', { muted_until: Math.floor(Date.now() / 1000) - 5 } as Partial<Server>)] })
      expect(document.querySelector('li.muted')).toBeNull()
    })

    it('offers four ways to mute and sends the end as epoch seconds', async () => {
      await open(three)
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      const ways = shown().spec.items.find((i) => i.id === 'mute')!.items!
      expect(ways.map((i) => i.text)).toEqual(['For 1 hour', 'For 8 hours', 'Until tomorrow', 'Until I turn it back on'])
      const now = Math.floor(Date.now() / 1000)
      pick('mute-0')
      pick('mute-3')
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('mute-1')
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('mute-2')
      const sent = calls('rail_edit') as { op: string, until: number }[]
      expect(sent.map((c) => c.op)).toEqual(['mute', 'mute', 'mute'])
      expect(sent[0]!.until).toBeGreaterThanOrEqual(now + 3600)
      expect(sent[0]!.until).toBeLessThan(now + 3700)
      expect(sent[1]!.until).toBeGreaterThanOrEqual(now + 8 * 3600)
      expect(sent[2]!.until).toBeGreaterThan(now + 60)
      expect(sent[2]!.until).toBeLessThanOrEqual(now + 32 * 3600)
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('a.example: muted, Until tomorrow')
    })

    it('forever is -1, and a muted server is unmuted with 0', async () => {
      await open({ ...three, instances: [server('a'), server('b', { muted_until: -1 } as Partial<Server>)] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('mute-3')
      expect((calls('rail_edit')[0] as { until: number }).until).toBe(-1)
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).not.toContain('Mute server')
      pick('unmute')
      expect(calls('rail_edit')[1]).toEqual({ op: 'mute', id: 'b', until: 0 })
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('b.example: unmuted')
    })

    it('marks everything as read on a server that is running, and offers it for no other', async () => {
      await open({ ...three, instances: [server('a'), server('b', { status: 'refused', message: 'x' })] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      pick('mark-read')
      expect(calls('rail_edit')).toEqual([{ op: 'mark_read', id: 'a' }])
      rightClick(tiles()[1]!)
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).not.toContain('Mark all as read')
    })

    it('opens the switcher over the servers in the order of the rail and switches to the one chosen', async () => {
      await open({ ...three, instances: [server('a', { alias: 'Home', name: 'Home' }), server('b'), server('c')], items: [only('a'), folder('f-1', ['b', 'c'], { name: 'Work' })] })
      answer({ rail_dialog: { button: 'pick', value: 'c', color: null } })
      rail().quickSwitch()
      await vi.advanceTimersByTimeAsync(0)
      const spec = JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec)
      expect(spec.list.choices).toEqual([{ id: 'a', text: 'Home (a.example)', hint: 'a.example' }, { id: 'b', text: 'b.example', hint: 'Work' }, { id: 'c', text: 'c.example', hint: 'Work' }])
      expect(spec.buttons.map((b: { id: string }) => b.id)).toEqual(['pick', 'cancel'])
      expect(calls('rail_activate')).toEqual([{ id: 'c' }])
    })

    it('does nothing when the switcher is cancelled, and has nothing to show without servers', async () => {
      await open(three)
      answer({ rail_dialog: { button: 'cancel', value: 'c', color: null } })
      rail().quickSwitch()
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_activate')).toHaveLength(0)
      await open({ active: null, adding: true, can_add: true, instances: [] })
      rail().quickSwitch()
      expect(calls('rail_dialog')).toHaveLength(0)
    })

    it('exports the list and tells where it went', async () => {
      await open(three, 'en-US', { rail_edit: '/home/me/Downloads/mnema-servers.json', rail_dialog: { button: 'ok', value: null, color: null } })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      expect(texts(shown().spec.items)).toEqual(['Export server list…', 'Import server list…', 'Unload idle servers'])
      pick('export')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toEqual([{ op: 'export' }])
      expect(JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec).message).toBe('Saved to /home/me/Downloads/mnema-servers.json')
    })

    it('offers when idle servers unload, checks the one in force, and sets another', async () => {
      await open({ ...three, unload_after: 600 })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      const unload = shown().spec.items.find((i) => i.id === 'unload')!
      expect(unload.text).toBe('Unload idle servers')
      expect(unload.items!.map((i) => [i.text, i.checked])).toEqual([['Off', false], ['After 10 minutes', true], ['After 1 hour', false]])
      pick('unload-2')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toEqual([{ op: 'unload_after', until: 3600 }])
      expect(live()).toBe('Unload idle servers: After 1 hour')
    })

    it('says the choices in German and checks Off when the host closes no page', async () => {
      await open({ ...three, unload_after: null }, 'de-DE')
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      const unload = shown().spec.items.find((i) => i.id === 'unload')!
      expect(unload.text).toBe('Inaktive Server entladen')
      expect(unload.items!.map((i) => [i.text, i.checked])).toEqual([['Aus', true], ['Nach 10 Minuten', false], ['Nach 1 Stunde', false]])
      pick('unload-0')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toEqual([{ op: 'unload_after', until: -1 }])
    })

    it('says so when the export fails', async () => {
      await open(three)
      answer({ rail_edit: () => { throw new Error('no') } })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      pick('export')
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('The list could not be exported')
    })

    it('imports the file the person picked and reports how many servers it added', async () => {
      await open(three)
      answer({ rail_edit: 2, rail_dialog: { button: 'import', value: '{"version":1}', color: null } })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      pick('import')
      await vi.advanceTimersByTimeAsync(0)
      const spec = JSON.parse((calls('rail_dialog')[0] as { spec: string }).spec)
      expect(spec.file).toEqual({ label: 'File (.json)', accept: '.json', max: 8388608 })
      expect(calls('rail_edit')).toEqual([{ op: 'import', text: '{"version":1}' }])
      expect(live()).toBe('2 servers added')
    })

    it('does not import when the question is cancelled, and says so when the host refuses the file', async () => {
      await open(three)
      answer({ rail_dialog: { button: 'cancel', value: null, color: null } })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      pick('import')
      await vi.advanceTimersByTimeAsync(0)
      expect(calls('rail_edit')).toHaveLength(0)
      answer({ rail_edit: () => { throw new Error('bad') }, rail_dialog: { button: 'import', value: 'x', color: null } })
      add().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
      await vi.advanceTimersByTimeAsync(0)
      pick('import')
      await vi.advanceTimersByTimeAsync(0)
      expect(live()).toBe('The list could not be imported')
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
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke: vi.fn(async () => { throw new Error('gone') }) }, configurable: true })
    await runPage()
    expect(tiles()).toHaveLength(0)
    expect(add().textContent).toBe('+')
  })
})
