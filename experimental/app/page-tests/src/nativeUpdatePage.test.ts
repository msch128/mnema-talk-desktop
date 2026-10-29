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

  // Every phase and flag combination the host can send, with what the page shows.
  const views: Array<[string, Partial<Snapshot> & { external?: boolean; portable?: boolean }, { detail: string; buttons: string[]; skip: boolean; notes: boolean; bar: boolean; err: boolean }]> = [
    ['idle', {}, { detail: 'Checking whether a newer version exists.', buttons: ['Check again', 'Close'], skip: false, notes: false, bar: false, err: false }],
    ['checking', { phase: 'checking' }, { detail: 'This only takes a moment.', buttons: [], skip: false, notes: false, bar: true, err: false }],
    ['up to date', { phase: 'up_to_date', notes: 'n' }, { detail: 'No newer version is available.', buttons: ['Check again', 'Close'], skip: false, notes: false, bar: false, err: false }],
    ['available', { phase: 'available', version: '2', notes: 'n' }, { detail: 'Version 2 is available. The update is signed and verified before it is installed.', buttons: ['Update now', 'On next start'], skip: true, notes: true, bar: false, err: false }],
    ['available, package managed', { phase: 'available', version: '2', external: true }, { detail: 'Version 2 is available. This installation is managed by your package manager: download the new package from the release page.', buttons: ['Open release page', 'Close'], skip: true, notes: false, bar: false, err: false }],
    ['available, portable', { phase: 'available', version: '2', portable: true, external: true }, { detail: 'Version 2 is available. This portable copy does not update itself: download the new portable ZIP from the release page.', buttons: ['Open release page', 'Close'], skip: true, notes: false, bar: false, err: false }],
    ['downloading now', { phase: 'downloading', version: '2', mode: 'now', received: 1, total: 4 }, { detail: 'Downloading version 2. The app restarts afterwards.', buttons: [], skip: false, notes: false, bar: true, err: false }],
    ['downloading later', { phase: 'downloading', version: '2', mode: 'later' }, { detail: 'Downloading version 2 in the background. You can keep working.', buttons: [], skip: false, notes: false, bar: true, err: false }],
    ['ready', { phase: 'ready', version: '2', notes: 'n' }, { detail: 'Version 2 is downloaded and installs the next time Mnema Desktop starts.', buttons: ['Install now', 'Close'], skip: false, notes: true, bar: false, err: false }],
    ['installing', { phase: 'installing' }, { detail: 'The app closes shortly and restarts.', buttons: [], skip: false, notes: false, bar: true, err: false }],
    ['failed with a version', { phase: 'failed', version: '2', error: 'untrusted', notes: 'n' }, { detail: 'The current trust statement could not be fetched. Check your connection and try again.', buttons: ['Retry', 'Close'], skip: true, notes: false, bar: false, err: true }],
    ['failed on a managed install', { phase: 'failed', version: '2', external: true, mode: 'now' }, { detail: 'Something went wrong while updating.', buttons: ['Retry', 'Close'], skip: true, notes: false, bar: false, err: true }],
    ['failed on a portable copy', { phase: 'failed', version: '2', portable: true, error: 'install' }, { detail: 'Installation failed. The installed version is unchanged.', buttons: ['Retry', 'Close'], skip: true, notes: false, bar: false, err: true }],
    ['failed without a version', { phase: 'failed', error: 'offline' }, { detail: 'The update server cannot be reached. Check your connection.', buttons: ['Retry', 'Close'], skip: false, notes: false, bar: false, err: true }]
  ]

  it.each(views)('shows the %s state', async (_name, snapshot, want) => {
    await open(snapshot)
    expect(text('detail')).toBe(want.detail)
    expect(buttons()).toEqual(want.buttons)
    expect(document.getElementById('skip')!.hidden).toBe(!want.skip)
    expect(document.getElementById('notes')!.hidden).toBe(!want.notes)
    expect(document.getElementById('bar')!.hidden).toBe(!want.bar)
    expect(document.querySelector('.mark')!.classList.contains('err')).toBe(want.err)
  })

  it('sends the retry and download actions of the managed states', async () => {
    const first = async (snapshot: Partial<Snapshot> & { external?: boolean; portable?: boolean }) => {
      await open(snapshot)
      invoke.mockClear()
      document.querySelector<HTMLButtonElement>('#actions button')!.click()
      return invoke.mock.calls.filter((c) => c[0] === 'native_update_action').map((c) => c[1])
    }
    expect(await first({ phase: 'available', version: '2', external: true })).toEqual([{ action: 'download_page' }])
    expect(await first({ phase: 'available', version: '2', portable: true })).toEqual([{ action: 'download_page' }])
    expect(await first({ phase: 'failed', version: '2', external: true, mode: 'later' })).toEqual([{ action: 'check' }])
    expect(await first({ phase: 'failed', version: '2', portable: true })).toEqual([{ action: 'check' }])
    expect(await first({ phase: 'ready', version: '2' })).toEqual([{ action: 'now' }])
  })

  it('links to the release page through the host, in every phase', async () => {
    for (const snapshot of [{}, { phase: 'available', version: '2' }, { phase: 'available', version: '2', portable: true }, { phase: 'failed', error: 'offline' }]) {
      await open(snapshot)
      const link = document.getElementById('release') as HTMLButtonElement
      expect(link.hidden).toBe(false)
      expect(link.textContent).toBe('Release notes / download page')
      invoke.mockClear()
      link.click()
      expect(invoke.mock.calls.filter((c) => c[0] === 'native_update_action').map((c) => c[1])).toEqual([{ action: 'release_page' }])
    }
    await open({}, 'de-DE')
    expect(text('release')).toBe('Release-Hinweise und Download-Seite')
  })

  it('lets a failure be selected and copied with its technical detail', async () => {
    const writeText = vi.fn(async () => undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    await open({ phase: 'available', version: '2' })
    expect(document.getElementById('copy')!.hidden).toBe(true)
    await open({ phase: 'failed', version: '2', error: 'verify', mode: 'now', portable: true })
    const copy = document.getElementById('copy') as HTMLButtonElement
    expect(copy.hidden).toBe(false)
    expect(copy.textContent).toBe('Copy error')
    copy.click()
    await vi.advanceTimersByTimeAsync(0)
    expect(writeText).toHaveBeenCalledOnce()
    const copied = (writeText.mock.calls[0] as unknown as [string])[0]
    expect(copied).toContain('The update signature does not match. It was not installed.')
    expect(copied).toContain('error=verify')
    expect(copied).toContain('installed=v0.1.0')
    expect(copied).toContain('version=2')
    expect(copy.textContent).toBe('Copied')
    await vi.advanceTimersByTimeAsync(2100)
    expect(copy.textContent).toBe('Copy error')
    // The sentence itself is selectable text.
    expect(readFileSync(resolve(dir, 'update.css'), 'utf8')).toMatch(/\.detail \{[^}]*user-select: text/)
  })

  it('falls back to the selection command where the clipboard API is missing', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async () => { throw new Error('denied') } }, configurable: true })
    const exec = vi.fn(() => true)
    Object.defineProperty(document, 'execCommand', { value: exec, configurable: true })
    await open({ phase: 'failed', error: 'offline' })
    document.getElementById('copy')!.click()
    await vi.advanceTimersByTimeAsync(0)
    expect(exec).toHaveBeenCalledWith('copy')
    expect(text('copy')).toBe('Copied')
  })

  it('ships no inline script or handler, and loads nothing remote', () => {
    expect(html).not.toMatch(/<script(?![^>]*\ssrc=)|\son\w+=|https?:\/\//i)
    expect(script).not.toMatch(/fetch\(|XMLHttpRequest|innerHTML|eval\(/)
  })
})
