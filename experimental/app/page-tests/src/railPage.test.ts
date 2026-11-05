// The server rail (experimental/app/rail-ui): tiles, menu, notification level, removal and names.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { add, answer, calls, flat, folder, invoke, key, live, only, open, pick, rail, rightClick, server, shown, texts, three, tiles } from './railHarness.ts'

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
    // A server that cannot be reached shows a dot, not a count.
    expect(items[2]!.querySelector('.badge')).toBeNull()
    expect(items[2]!.querySelector('.dot')).not.toBeNull()
    expect(items[1]!.querySelector('.dot')).toBeNull()
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
      expect(texts(shown().spec.items)).toEqual(['a.example', 'Mark all as read', 'Mute server', 'Rename…', 'Add to folder', 'Move up', 'Move down', 'Remove…'])
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

    it('cuts long names between characters, never inside one', async () => {
      await open({ ...three, instances: [server('a', { name: '😀'.repeat(70), original: '😀'.repeat(70) })] })
      rightClick(tiles()[0]!)
      await vi.advanceTimersByTimeAsync(0)
      const title = shown().spec.items[0]!.text!
      expect(title.isWellFormed()).toBe(true)
      expect([...title]).toHaveLength(60)
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
      expect(texts(shown().spec.items)).toEqual(['a.example', 'Benachrichtigungen', 'Alles als gelesen markieren', 'Server stummschalten', 'Umbenennen …', 'Zu Ordner hinzufügen', 'Nach oben', 'Nach unten', 'Entfernen …'])
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
})
