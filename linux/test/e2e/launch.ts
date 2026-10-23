import { type ChildProcess, spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Page, type Target, pages } from './cdp.ts'

const root = new URL('../..', import.meta.url).pathname

export interface Running {
  port: number
  dataDir: string
  output: string[]
  child: ChildProcess
  /** The first page that satisfies `match`, waiting for it to appear. */
  page(match: (target: Target) => boolean, timeoutMs?: number): Promise<Page>
  /** The first page of the app (its own pages, not the DevTools') for which `check` holds, evaluated inside it. */
  pageWhere(check: (page: Page) => Promise<boolean>, timeoutMs?: number): Promise<Page>
  /** The page that runs the bundle in the given role (`selector`, `instance`) or the server rail (`rail`). */
  role(role: 'rail' | 'selector' | 'instance', timeoutMs?: number): Promise<Page>
  stop(): Promise<void>
}

/** Launches the unpackaged app (the `electron` binary of node_modules) with remote debugging on a free port. */
export async function launch(env: Record<string, string> = {}, args: string[] = []): Promise<Running> {
  const dataDir = mkdtempSync(join(tmpdir(), 'mnema-e2e-'))
  // MNEMA_E2E_APP runs a packaged build (the executable of `release/linux-unpacked`) instead of the development app.
  const packaged = process.env.MNEMA_E2E_APP
  const executable = packaged ?? ((await import('electron')) as unknown as { default: string }).default
  const output: string[] = []
  const child = spawn(executable, [...(packaged ? [] : [root]), '--remote-debugging-port=0', ...args], {
    env: { ...process.env, MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING: '1', MNEMA_DESKTOP_DATA_DIR: dataDir, MNEMA_DESKTOP_NO_UPDATE_CHECK: '1', MNEMA_DESKTOP_NO_VAULT: '1', MNEMA_DEBUG_PROTOCOL: '1', ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const port = await new Promise<number>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`the app did not start:\n${output.join('')}`)), 30_000)
    const read = (chunk: Buffer) => {
      const text = chunk.toString()
      output.push(text)
      const match = /DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)\//.exec(text)
      if (match) { clearTimeout(timer); resolve(Number(match[1])) }
    }
    child.stdout?.on('data', read)
    child.stderr?.on('data', read)
    child.once('exit', (code) => { clearTimeout(timer); reject(new Error(`the app exited with ${code}:\n${output.join('')}`)) })
  })
  const open: Page[] = []
  const running: Running = {
    port, dataDir, output, child,
    async page(match, timeoutMs = 20_000) {
      const deadline = Date.now() + timeoutMs
      for (;;) {
        const target = (await pages(port)).find(match)
        if (target) {
          const page = new Page(target)
          open.push(page)
          await page.opened
          return page
        }
        if (Date.now() > deadline) throw new Error(`no page appeared:\n${output.join('')}`)
        await new Promise((resolve) => setTimeout(resolve, 200))
      }
    },
    async pageWhere(check, timeoutMs = 20_000) {
      const deadline = Date.now() + timeoutMs
      for (;;) {
        for (const target of await pages(port)) {
          if (!target.url.startsWith('mnema://localhost')) continue
          const page = new Page(target)
          await page.opened
          if (await check(page).catch(() => false)) { open.push(page); return page }
          page.close()
        }
        if (Date.now() > deadline) throw new Error(`no matching page appeared:\n${output.join('')}`)
        await new Promise((resolve) => setTimeout(resolve, 300))
      }
    },
    async role(role, timeoutMs) {
      return running.pageWhere(async (page) => (await page.evaluate<string | null>("window.__MNEMA_RAIL_HOST__ ? 'rail' : (window.__MNEMA_TRUSTED_APP__ ? window.__MNEMA_TRUSTED_APP__.role : null)")) === role, timeoutMs)
    },
    async stop() {
      for (const page of open) page.close()
      child.removeAllListeners('exit')
      child.kill('SIGTERM')
      await new Promise<void>((resolve) => { if (child.exitCode !== null) resolve(); else child.once('exit', () => resolve()) })
      rmSync(dataDir, { recursive: true, force: true })
    },
  }
  return running
}
