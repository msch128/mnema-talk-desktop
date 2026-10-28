// The host's own update page (experimental/app/update-ui), run against a fake
// native bridge. It is plain script served by the executable, so it is tested
// here.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Vitest runs from the page-tests directory.
const dir = resolve(process.cwd(), '../update-ui')
const html = readFileSync(resolve(dir, 'index.html'), 'utf8')
const script = readFileSync(resolve(dir, 'update.js'), 'utf8')

interface Snapshot {
  current: string
  phase: string
  version: string | null
  notes: string | null
  received: number
  total: number | null
  mode: string | null
  error: string | null
}

const base: Snapshot = { current: '0.1.0', phase: 'idle', version: null, notes: null, received: 0, total: null, mode: null, error: null }
let state: Snapshot
let invoke: ReturnType<typeof vi.fn>

function language(value: string) {
  Object.defineProperty(navigator, 'language', { value, configurable: true })
}

// Parse the page into an inert template and drop the stylesheet and script elements
// the executable loads itself; they are not fetched in the test window.
function pageMarkup(): string {
  const template = document.createElement('template')
  template.innerHTML = html
  template.content.querySelectorAll('link, script').forEach((el) => el.remove())
  return template.innerHTML
}

async function open(snapshot: Partial<Snapshot>, lang = 'en-US') {
  state = { ...base, ...snapshot }
  language(lang)
  // The stylesheet and script files are loaded by the executable, not fetched here.
  document.documentElement.innerHTML = pageMarkup()
  invoke = vi.fn(async (cmd: string) => (cmd === 'native_update_state' || cmd === 'native_update_action' ? state : undefined))
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke }, configurable: true })
  // The page script is plain browser code; run it in the test window.
  // eslint-disable-next-line no-new-func
  new Function(script)()
  await vi.advanceTimersByTimeAsync(0)
}

const text = (id: string) => document.getElementById(id)!.textContent
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>('#actions button')].map((b) => b.textContent)

describe('update page', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('offers both ways to update and a skip in English', async () => {
    await open({ phase: 'available', version: '0.2.0', notes: 'Fixes' })
    expect(text('title')).toBe('Update available')
    expect(text('detail')).toContain('0.2.0')
    expect(buttons()).toEqual(['Update now', 'On next start'])
    expect(document.getElementById('skip')!.hidden).toBe(false)
    expect(text('skip')).toBe('Skip this version')
    expect(text('version')).toBe('Installed: v0.1.0')
    expect(document.getElementById('notes')!.hidden).toBe(false)
  })

  it('speaks German on a German system', async () => {
    await open({ phase: 'available', version: '0.2.0' }, 'de-DE')
    expect(text('title')).toBe('Update verfügbar')
    expect(buttons()).toEqual(['Jetzt aktualisieren', 'Beim nächsten Start'])
    expect(text('skip')).toBe('Diese Version überspringen')
    expect(document.documentElement.lang).toBe('de')
    await open({ phase: 'failed', error: 'verify' }, 'de')
    expect(text('detail')).toContain('Signatur')
  })

  it('sends the chosen action to the host', async () => {
    await open({ phase: 'available', version: '0.2.0' })
    const [now, later] = [...document.querySelectorAll<HTMLButtonElement>('#actions button')]
    now!.click()
    later!.click()
    document.getElementById('skip')!.click()
    expect(invoke.mock.calls.filter((c) => c[0] === 'native_update_action').map((c) => c[1])).toEqual([
      { action: 'now' },
      { action: 'later' },
      { action: 'skip' }
    ])
  })

  it('shows download progress, indeterminate until the size is known', async () => {
    await open({ phase: 'downloading', version: '0.2.0', mode: 'now', received: 25, total: 100 })
    expect(document.getElementById('bar')!.hidden).toBe(false)
    expect(document.getElementById('fill')!.style.width).toBe('25%')
    expect(document.getElementById('bar')!.getAttribute('aria-valuenow')).toBe('25')
    expect(buttons()).toEqual([])
    await open({ phase: 'downloading', version: '0.2.0', mode: 'later', received: 25, total: null })
    expect(document.getElementById('bar')!.classList.contains('indeterminate')).toBe(true)
    expect(text('detail')).toContain('background')
  })

  it('follows the host while a download runs', async () => {
    await open({ phase: 'downloading', version: '0.2.0', mode: 'now', received: 10, total: 100 })
    state = { ...state, received: 80 }
    await vi.advanceTimersByTimeAsync(600)
    expect(document.getElementById('fill')!.style.width).toBe('80%')
    state = { ...state, phase: 'ready', mode: 'later' }
    await vi.advanceTimersByTimeAsync(600)
    expect(text('title')).toBe('Update ready')
    expect(buttons()).toEqual(['Install now', 'Close'])
  })

  it('explains a failure without raw error text and offers a retry', async () => {
    for (const [error, word] of [['offline', 'cannot be reached'], ['verify', 'signature'], ['install', 'unchanged'], ['unknown', 'wrong']] as const) {
      await open({ phase: 'failed', error, version: '0.2.0', mode: 'now' })
      expect(text('detail')).toContain(word)
      expect(buttons()).toEqual(['Retry', 'Close'])
      expect(document.querySelector('.mark')!.classList.contains('err')).toBe(true)
    }
    await open({ phase: 'failed', error: 'offline' })
    invoke.mockClear()
    document.querySelector<HTMLButtonElement>('#actions button')!.click()
    expect(invoke).toHaveBeenCalledWith('native_update_action', { action: 'check' })
    expect(document.getElementById('skip')!.hidden).toBe(true)
  })

  it('retries the same way it failed', async () => {
    await open({ phase: 'failed', error: 'offline', version: '0.2.0', mode: 'later' })
    invoke.mockClear()
    document.querySelector<HTMLButtonElement>('#actions button')!.click()
    expect(invoke).toHaveBeenCalledWith('native_update_action', { action: 'later' })
  })

  it('shows release notes as plain text', async () => {
    await open({ phase: 'available', version: '0.2.0', notes: '<img src=x onerror=alert(1)><b>x</b>' })
    expect(document.querySelector('#notes img')).toBeNull()
    expect(document.querySelector('#notes b')).toBeNull()
    expect(text('notes')).toContain('<img')
  })

  it('stays calm when up to date or idle', async () => {
    await open({ phase: 'up_to_date' })
    expect(text('title')).toBe('You are up to date')
    expect(buttons()).toEqual(['Check again', 'Close'])
    expect(document.getElementById('skip')!.hidden).toBe(true)
    await open({ phase: 'checking' })
    expect(document.getElementById('bar')!.hidden).toBe(false)
    expect(buttons()).toEqual([])
  })

  it('does not rebuild the buttons when nothing changed', async () => {
    await open({ phase: 'available', version: '0.2.0' })
    const first = document.querySelector('#actions button')
    await vi.advanceTimersByTimeAsync(1600)
    expect(document.querySelector('#actions button')).toBe(first)
  })

  it('ships no inline script or handler, and loads nothing remote', () => {
    expect(html).not.toMatch(/<script(?![^>]*\ssrc=)|\son\w+=|https?:\/\//i)
    expect(script).not.toMatch(/fetch\(|XMLHttpRequest|innerHTML|eval\(/)
  })
})
