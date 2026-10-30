// The question of the server rail (experimental/app/rail-ui/dialog.html), run
// against a fake host. It is plain script served by the executable (and by the
// Linux client, unchanged).
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const dir = resolve(process.cwd(), '../rail-ui')
const html = readFileSync(resolve(dir, 'dialog.html'), 'utf8')
const script = readFileSync(resolve(dir, 'dialog.js'), 'utf8')

interface Spec {
  title: string
  message?: string
  input?: { label: string, value: string, max: number, placeholder?: string }
  colors?: { label: string, value: string, choices: { value: string, label: string }[] }
  buttons: { id: string, text: string, primary?: boolean, danger?: boolean }[]
  cancel: string
}

const rename: Spec = {
  title: 'Rename server',
  message: 'Only on this device.',
  input: { label: 'Name', value: 'Lantern', max: 40, placeholder: 'a.example' },
  buttons: [{ id: 'save', text: 'Save', primary: true }, { id: 'cancel', text: 'Cancel' }],
  cancel: 'cancel',
}
const folder: Spec = {
  title: 'Edit folder',
  input: { label: 'Folder name', value: '', max: 40 },
  colors: { label: 'Colour', value: '#3b82f6', choices: [{ value: '', label: 'None' }, { value: '#2da771', label: 'Green' }, { value: '#3b82f6', label: 'Blue' }] },
  buttons: [{ id: 'save', text: 'Save', primary: true }, { id: 'cancel', text: 'Cancel' }],
  cancel: 'cancel',
}
const remove: Spec = {
  title: 'Remove server: a.example',
  message: 'Removes everything.',
  buttons: [{ id: 'remove', text: 'Remove server', danger: true }, { id: 'cancel', text: 'Cancel', primary: true }],
  cancel: 'cancel',
}

let done: ReturnType<typeof vi.fn>

function open(spec: Spec, how: 'bridge' | 'tauri' = 'bridge') {
  const template = document.createElement('template')
  template.innerHTML = html
  template.content.querySelectorAll('link, script').forEach((el) => el.remove())
  document.documentElement.innerHTML = template.innerHTML
  done = vi.fn(async () => undefined)
  if (how === 'bridge') Object.defineProperty(window, '__MNEMA_DIALOG_HOST__', { value: { spec, done }, configurable: true })
  else {
    Object.defineProperty(window, '__MNEMA_DIALOG__', { value: spec, configurable: true })
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: { invoke: done }, configurable: true })
  }
  // eslint-disable-next-line no-new-func
  new Function(script)()
}

const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>('#actions button')].find((b) => b.textContent === text)!
const input = () => document.getElementById('input') as HTMLInputElement
const swatches = () => [...document.querySelectorAll<HTMLButtonElement>('.swatch')]
const press = (el: Element, key: string) => el.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }))

describe('rail question', () => {
  afterEach(() => {
    Reflect.deleteProperty(window, '__MNEMA_DIALOG_HOST__')
    Reflect.deleteProperty(window, '__MNEMA_DIALOG__')
  })

  it('shows the title, the note and the field with the current value selected', () => {
    open(rename)
    expect(document.title).toBe('Rename server')
    expect(document.getElementById('title')!.textContent).toBe('Rename server')
    expect(document.getElementById('message')!.hidden).toBe(false)
    expect(document.getElementById('label')!.textContent).toBe('Name')
    expect(input().value).toBe('Lantern')
    expect(input().maxLength).toBe(40)
    expect(input().placeholder).toBe('a.example')
    expect(document.activeElement).toBe(input())
    expect(document.getElementById('colors')!.hidden).toBe(true)
  })

  it('answers with the button and the text, once', () => {
    open(rename)
    input().value = 'Home'
    button('Save').click()
    button('Save').click()
    press(document.body, 'Escape')
    expect(done).toHaveBeenCalledTimes(1)
    expect(done).toHaveBeenCalledWith({ button: 'save', value: 'Home', color: null })
  })

  it('saves with Enter in the field and cancels with Escape', () => {
    open(rename)
    press(input(), 'Enter')
    expect(done).toHaveBeenCalledWith({ button: 'save', value: 'Lantern', color: null })
    open(rename)
    press(input(), 'Escape')
    expect(done).toHaveBeenCalledWith({ button: 'cancel', value: 'Lantern', color: null })
  })

  it('has no field to answer with when it asks only a question, and puts focus on the safe button', () => {
    open(remove)
    expect(document.getElementById('field')!.hidden).toBe(true)
    expect(document.activeElement).toBe(button('Cancel'))
    expect(button('Remove server').classList.contains('danger')).toBe(true)
    button('Remove server').click()
    expect(done).toHaveBeenCalledWith({ button: 'remove', value: null, color: null })
  })

  it('lets Enter press the button that has the focus instead of the main one', () => {
    open(remove)
    press(button('Cancel'), 'Enter')
    expect(done).not.toHaveBeenCalled()
  })

  it('offers the colours as one choice, the current one selected', () => {
    open(folder)
    expect(swatches().map((s) => s.getAttribute('aria-checked'))).toEqual(['false', 'false', 'true'])
    expect(swatches().map((s) => s.tabIndex)).toEqual([-1, -1, 0])
    expect(swatches().map((s) => s.getAttribute('aria-label'))).toEqual(['None', 'Green', 'Blue'])
    expect(swatches()[0]!.classList.contains('none')).toBe(true)
    expect(swatches()[1]!.style.getPropertyValue('--swatch')).toBe('#2da771')
  })

  it('picks a colour with the pointer or the arrow keys, and answers with it', () => {
    open(folder)
    swatches()[1]!.click()
    expect(swatches().map((s) => s.getAttribute('aria-checked'))).toEqual(['false', 'true', 'false'])
    press(swatches()[1]!, 'ArrowRight')
    expect(document.activeElement).toBe(swatches()[2])
    press(swatches()[2]!, 'ArrowRight')
    expect(document.activeElement).toBe(swatches()[0])
    input().value = 'Friends'
    button('Save').click()
    expect(done).toHaveBeenCalledWith({ button: 'save', value: 'Friends', color: '' })
  })

  it('shows text as text, never as markup', () => {
    open({ ...remove, title: '<img src=x onerror=alert(1)>', message: '<b>bold</b>' })
    expect(document.querySelector('img, b')).toBeNull()
    expect(document.getElementById('title')!.textContent).toBe('<img src=x onerror=alert(1)>')
  })

  it('talks to the host through the bridge of Tauri when no other is given', () => {
    open(rename, 'tauri')
    button('Save').click()
    expect(done).toHaveBeenCalledWith('dialog_done', { button: 'save', value: 'Lantern', color: null })
  })

  it('does not fail when the host cannot be reached', () => {
    open(rename)
    done.mockImplementation(() => { throw new Error('gone') })
    expect(() => button('Save').click()).not.toThrow()
  })
})
