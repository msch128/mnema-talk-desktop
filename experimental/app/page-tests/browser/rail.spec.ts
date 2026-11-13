// The server rail with many servers, in WebKit and Chromium, at 100 % and
// 200 % scale: dragging near an edge scrolls the list, the server in front
// stops clear of the fade, and a "99+" badge fits inside the rail.
import { type Page, expect, test } from '@playwright/test'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

const page_url = pathToFileURL(resolve(process.cwd(), '../rail-ui/index.html')).href
const RAIL = 68

interface Server { id: string, name: string, original: string, alias: null, initial: string, hue: number, status: string, message: null, unread: number, mentions: number, icon: null }
const server = (id: string, extra: Partial<Server> = {}): Server => ({ id, name: `${id}.example`, original: `${id}.example`, alias: null, initial: id[0]!.toUpperCase(), hue: 120, status: 'ready', message: null, unread: 0, mentions: 0, icon: null, ...extra })
const many = (n: number, active: string, extra: (i: number) => Partial<Server> = () => ({})) => ({ active, adding: false, can_add: true, instances: Array.from({ length: n }, (_, i) => server(`s${i}`, extra(i))) })

async function openRail(page: Page, state: unknown) {
  await page.addInitScript((initial) => {
    const host = { invoke: (cmd: string) => Promise.resolve(cmd === 'rail_state' ? initial : null) }
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: host })
  }, state)
  await page.goto(page_url)
  await page.waitForSelector('.tile')
}

for (const scale of [1, 2]) {
  test.describe(`at ${scale * 100} %`, () => {
    test.use({ deviceScaleFactor: scale })

    test('a drag held near the bottom edge scrolls the list, and near the top back up', async ({ page }) => {
      await openRail(page, many(50, 's0'))
      const scrolled = await page.evaluate(async () => {
        const scroll = document.getElementById('scroll')!
        const tile = document.querySelector<HTMLElement>('.tile')!
        const data = new DataTransfer()
        tile.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: data }))
        const box = scroll.getBoundingClientRect()
        const at = (y: number) => scroll.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, clientX: 34, clientY: y, dataTransfer: data }))
        for (let i = 0; i < 20; i++) at(box.bottom - 4)
        const down = scroll.scrollTop
        for (let i = 0; i < 5; i++) at(box.top + 4)
        const back = scroll.scrollTop
        for (let i = 0; i < 5; i++) at(box.top + box.height / 2)
        const still = scroll.scrollTop
        tile.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: data }))
        return { down, back, still }
      })
      expect(scrolled.down).toBeGreaterThan(100)
      expect(scrolled.back).toBeLessThan(scrolled.down)
      expect(scrolled.still).toBe(scrolled.back)
    })

    test('the server in front, deep in a long list, is shown clear of the fade', async ({ page }) => {
      await openRail(page, many(50, 's30'))
      const seen = await page.evaluate(() => {
        const scroll = document.getElementById('scroll')!
        const box = scroll.getBoundingClientRect()
        const tile = document.querySelector<HTMLElement>('.tile[aria-current="true"]')!.getBoundingClientRect()
        return { below: box.bottom - tile.bottom, above: tile.top - box.top, fadedBelow: scroll.classList.contains('more-below') }
      })
      // Scrolled down to it: there is more below, and the tile stops a fade's height above that edge.
      expect(seen.fadedBelow).toBe(true)
      expect(seen.below).toBeGreaterThanOrEqual(27)
      expect(seen.above).toBeGreaterThan(0)
    })

    test('at the very end of the list the last server needs no clearance: nothing fades there', async ({ page }) => {
      await openRail(page, many(50, 's49'))
      const end = await page.evaluate(() => document.getElementById('scroll')!.classList.contains('more-below'))
      expect(end).toBe(false)
    })

    test('a "99+" badge fits inside the rail, uncut', async ({ page }) => {
      await openRail(page, many(3, 's0', (i) => ({ mentions: i === 1 ? 500 : 0 })))
      const badge = page.locator('.badge').first()
      await expect(badge).toHaveText('99+')
      const fits = await badge.evaluate((el, rail) => {
        const box = el.getBoundingClientRect()
        return { right: box.right, left: box.left, within: box.right <= rail - 1 && box.left >= 0, whole: el.scrollWidth <= el.clientWidth }
      }, RAIL)
      expect(fits.within, JSON.stringify(fits)).toBe(true)
      expect(fits.whole).toBe(true)
    })
  })
}
