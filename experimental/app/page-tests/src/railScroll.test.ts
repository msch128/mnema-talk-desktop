// The server rail with more servers than fit: the list scrolls, the add button
// stays in sight, and the server in front is scrolled into view when it changes.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { add, folder, only, open, rail, server, type State } from './railHarness'

const many = (n: number, active: string): State => ({
  active, adding: false, can_add: true,
  instances: Array.from({ length: n }, (_, i) => server(`s${i}`)),
})

describe('a long rail', () => {
  let scrolled: string[]
  beforeEach(() => {
    vi.useFakeTimers()
    scrolled = []
    Element.prototype.scrollIntoView = function (this: Element) { scrolled.push((this as HTMLElement).dataset.id ?? '') }
  })
  afterEach(() => { vi.useRealTimers() })

  it('keeps the add button outside the scrolling list', async () => {
    await open(many(40, 's0'))
    const scroll = document.getElementById('scroll')!
    expect(scroll.contains(document.getElementById('list'))).toBe(true)
    expect(scroll.contains(add())).toBe(false)
    expect(document.querySelectorAll('.tile')).toHaveLength(40)
  })

  it('scrolls the server in front into view when it changes, and only then', async () => {
    await open(many(40, 's0'))
    expect(scrolled).toEqual(['s0'])
    rail().set(many(40, 's39'))
    expect(scrolled).toEqual(['s0', 's39'])
    // A redraw for something else (a count) leaves the scrolling alone.
    const counted = many(40, 's39')
    counted.instances[3]!.mentions = 2
    rail().set(counted)
    expect(scrolled).toEqual(['s0', 's39'])
  })

  it('scrolls to the closed folder that holds the server in front', async () => {
    const state = many(30, 's25')
    state.items = [...state.instances.slice(0, 25).map((s) => only(s.id)), folder('f1', ['s25', 's26'], { collapsed: true }), ...state.instances.slice(27).map((s) => only(s.id))]
    await open(state)
    expect(scrolled).toEqual(['f1'])
  })

  it('fades the edges that have more beyond them', async () => {
    await open(many(40, 's0'))
    const scroll = document.getElementById('scroll')!
    Object.defineProperty(scroll, 'clientHeight', { value: 300, configurable: true })
    Object.defineProperty(scroll, 'scrollHeight', { value: 2400, configurable: true })
    scroll.scrollTop = 0
    scroll.dispatchEvent(new Event('scroll'))
    expect([scroll.classList.contains('more-above'), scroll.classList.contains('more-below')]).toEqual([false, true])
    scroll.scrollTop = 1000
    scroll.dispatchEvent(new Event('scroll'))
    expect([scroll.classList.contains('more-above'), scroll.classList.contains('more-below')]).toEqual([true, true])
    scroll.scrollTop = 2100
    scroll.dispatchEvent(new Event('scroll'))
    expect([scroll.classList.contains('more-above'), scroll.classList.contains('more-below')]).toEqual([true, false])
  })
})
