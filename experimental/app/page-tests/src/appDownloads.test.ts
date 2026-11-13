// The page shim that hands download links of the app's own API to the host
// (experimental/app/page-shim/app-downloads.js), against a fake bridge that
// records what the host would be asked.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

const script = readFileSync(resolve(process.cwd(), '../page-shim/app-downloads.js'), 'utf8')

const invoke = vi.fn(() => Promise.resolve('/Users/someone/Downloads/x'))

function link(attributes: Record<string, string>, child = false) {
  const a = document.createElement('a')
  for (const [name, value] of Object.entries(attributes)) a.setAttribute(name, value)
  a.textContent = 'Download'
  document.body.append(a)
  if (!child) return { link: a, target: a as Element }
  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  a.prepend(icon)
  return { link: a, target: icon as Element }
}

/** Whether the shim took the click over (it prevented the webview's own handling). */
let taken = false

function click(target: Element, init: MouseEventInit = {}) {
  taken = false
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0, ...init }))
  return { defaultPrevented: taken }
}

describe('download links of the app', () => {
  beforeAll(() => {
    ;(window as unknown as { happyDOM: { setURL(url: string): void } }).happyDOM.setURL('https://mnema.localhost/settings/account')
    ;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = { invoke }
    new Function(script)()
    // After the shim: note what it decided, then keep the test page from
    // following the link (happy-dom would navigate away from this document).
    document.addEventListener('click', (event) => {
      taken = event.defaultPrevented
      event.preventDefault()
    }, true)
  })

  beforeEach(() => {
    invoke.mockClear()
    document.body.replaceChildren()
  })

  it('hands the data export to the host instead of the webview', () => {
    const { target } = link({ href: '/api/users/me/export/download', download: '' })
    const event = click(target)
    expect(event.defaultPrevented).toBe(true)
    expect(invoke).toHaveBeenCalledWith('desktop_download', { path: '/api/users/me/export/download', name: '' })
  })

  it('takes a click on the icon inside the link and keeps the query and the chosen name', () => {
    const { target } = link({ href: 'https://mnema.localhost/api/media/abc?v=2', download: 'photo.png' }, true)
    expect(click(target).defaultPrevented).toBe(true)
    expect(invoke).toHaveBeenCalledWith('desktop_download', { path: '/api/media/abc?v=2', name: 'photo.png' })
  })

  it('leaves links that are not downloads of the app API to the webview', () => {
    const cases: Record<string, string>[] = [
      { href: '/api/users/me/export/download' },
      { href: 'blob:https://mnema.localhost/3f2a', download: 'codes.txt' },
      { href: 'https://example.org/api/file', download: '' },
      { href: '/settings', download: '' },
      { href: 'mailto:someone@example.org', download: '' },
    ]
    for (const attributes of cases) {
      document.body.replaceChildren()
      const { target } = link(attributes)
      expect(click(target).defaultPrevented, JSON.stringify(attributes)).toBe(false)
    }
    expect(invoke).not.toHaveBeenCalled()
  })

  it('leaves clicks with a modifier, other buttons and handled clicks alone', () => {
    for (const init of [{ metaKey: true }, { ctrlKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
      document.body.replaceChildren()
      const { target } = link({ href: '/api/users/me/export/download', download: '' })
      expect(click(target, init).defaultPrevented, JSON.stringify(init)).toBe(false)
    }
    const { link: a } = link({ href: '/api/users/me/export/download', download: '' })
    const handled = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
    handled.preventDefault()
    a.dispatchEvent(handled)
    expect(invoke).not.toHaveBeenCalled()
  })

  it('still runs the page handler of the link', () => {
    const { link: a } = link({ href: '/api/users/me/export/download', download: '' })
    const pageHandler = vi.fn()
    a.addEventListener('click', pageHandler)
    click(a)
    expect(pageHandler).toHaveBeenCalledTimes(1)
    expect(invoke).toHaveBeenCalledTimes(1)
  })
})
