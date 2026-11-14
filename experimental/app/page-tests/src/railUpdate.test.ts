// The update pill at the foot of the server rail (rail-ui/rail-update.js).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { calls, invoke, open, three } from './railHarness'

interface Update { current: string, phase: string, version: string | null, received: number, total: number | null, mode: string | null, error: string | null, skipped?: boolean, confirm_call?: boolean }
const base: Update = { current: '1.3.5', phase: 'idle', version: null, received: 0, total: null, mode: null, error: null }
const offered: Update = { ...base, phase: 'available', version: '1.3.6' }

const pill = () => document.getElementById('update') as HTMLButtonElement
const push = (s: Update) => (window as unknown as { mnemaRail: { update: (s: Update) => void } }).mnemaRail.update(s)

describe('update pill', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('shows nothing while there is nothing to update, or on a host without an updater', async () => {
    await open(three, 'en-US', { rail_update_state: base })
    expect(pill().hidden).toBe(true)
    expect(document.body.classList.contains('has-update')).toBe(false)
    await open(three)
    expect(pill().hidden).toBe(true)
  })

  it('offers "Update now" for a new version and sends it to the host on a click', async () => {
    await open(three, 'en-US', { rail_update_state: offered, rail_update_action: { ...offered, phase: 'downloading' } })
    expect(calls('rail_update_state')).toEqual([{ german: false }])
    expect(pill().hidden).toBe(false)
    expect(pill().textContent).toBe('Update')
    expect(pill().getAttribute('aria-label')).toBe('Update available: version 1.3.6. Update now')
    expect(document.body.classList.contains('has-update')).toBe(true)
    pill().click()
    expect(calls('rail_update_action')).toEqual([{ action: 'now' }])
  })

  it('tells the host the language and speaks German', async () => {
    await open(three, 'de-DE', { rail_update_state: offered })
    expect(calls('rail_update_state')).toEqual([{ german: true }])
    expect(pill().getAttribute('aria-label')).toBe('Update verfügbar: Version 1.3.6. Jetzt aktualisieren')
    push({ ...offered, phase: 'waiting' })
    expect(pill().textContent).toBe('Nach Talk')
  })

  it('follows a download with its progress and opens the window when clicked', async () => {
    await open(three, 'en-US', { rail_update_state: { ...offered, phase: 'downloading', received: 30, total: 100 } })
    expect(pill().textContent).toBe('30%')
    expect(pill().style.getPropertyValue('--fill')).toBe('30%')
    expect(pill().getAttribute('aria-label')).toBe('Downloading version 1.3.6, 30%')
    // The page asks again while the download runs.
    invoke.mockClear()
    await vi.advanceTimersByTimeAsync(1100)
    expect(calls('rail_update_state').length).toBe(1)
    pill().click()
    expect(calls('rail_update_action')).toEqual([{ action: 'open' }])
  })

  it('stops asking once nothing runs', async () => {
    await open(three, 'en-US', { rail_update_state: offered })
    invoke.mockClear()
    await vi.advanceTimersByTimeAsync(5000)
    expect(calls('rail_update_state')).toEqual([])
  })

  it('shows the wait for the end of a Talk, the install and a failure, each opening the window', async () => {
    await open(three, 'en-US', { rail_update_state: offered })
    for (const [phase, text, kind] of [['waiting', 'After call', 'busy'], ['installing', '…', 'busy'], ['failed', 'Retry', 'failed']] as const) {
      push({ ...offered, phase, error: phase === 'failed' ? 'offline' : null })
      expect(pill().textContent).toBe(text)
      expect(pill().classList.contains(kind)).toBe(true)
      expect(pill().dataset.action).toBe('open')
    }
  })

  it('stays away for a skipped version, a failed check and when the update is done', async () => {
    await open(three, 'en-US', { rail_update_state: { ...offered, skipped: true } })
    expect(pill().hidden).toBe(true)
    push({ ...base, phase: 'failed', error: 'offline' })
    expect(pill().hidden).toBe(true)
    push(offered)
    expect(pill().hidden).toBe(false)
    push({ ...base, phase: 'up_to_date' })
    expect(pill().hidden).toBe(true)
    expect(document.body.classList.contains('has-update')).toBe(false)
  })

  it('ignores a push that is not a state', async () => {
    await open(three, 'en-US', { rail_update_state: offered })
    push(null as unknown as Update)
    push({} as unknown as Update)
    expect(pill().textContent).toBe('Update')
  })
})
