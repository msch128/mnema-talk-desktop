/**
 * Wires the self-update (`core/selfupdate.ts`) to the application: the pinned
 * key and feed, the trust statement, the AppImage that gets replaced, and the
 * restart.
 */
import { app } from 'electron'
import { UPDATER_ENDPOINTS, UPDATER_PUBKEY } from '../core/pinned.ts'
import { SelfUpdater, POLL_MS, menuText, platformKey, replaceAppImage } from '../core/selfupdate.ts'
import { updatesDisabled } from '../core/updates.ts'
import type { Context } from './context.ts'
import { buildMenu } from './menu.ts'
import { openUpdater } from './windows.ts'

/** The AppImage this process was started from; set by the AppImage runtime. */
function appImagePath(): string | null {
  const path = process.env.APPIMAGE
  return path && path.length > 0 ? path : null
}

export function createUpdater(ctx: Context, onRestart: () => void): SelfUpdater {
  const image = appImagePath()
  let shown = ''
  return new SelfUpdater({
    current: app.getVersion(),
    dir: app.getPath('userData'),
    endpoints: UPDATER_ENDPOINTS.map((e) => new URL(e)),
    pubkey: UPDATER_PUBKEY,
    platform: platformKey(),
    statement: () => ctx.trusted.statement(),
    installsAllowed: () => ctx.trusted.fresh(),
    trustRound: () => ctx.trusted.trustRound(),
    install: (bytes) => {
      if (!image) return Promise.reject(new Error('not an AppImage'))
      replaceAppImage(image, bytes)
      return Promise.resolve()
    },
    restart: () => {
      onRestart()
      if (image) app.relaunch({ execPath: image, args: process.argv.slice(1) })
      else app.relaunch()
      app.exit(0)
    },
    now: () => Math.floor(Date.now() / 1000),
    githubOnly: true,
    external: image === null,
    onChange: (snapshot) => {
      // The menu says only whether something waits; progress does not rebuild it.
      const label = menuText(snapshot.phase)
      if (label !== shown) { shown = label; buildMenu(ctx) }
    },
    announce: () => { openUpdater(ctx, false) },
  })
}

/** A check at every start, then whenever the interval has passed. */
export function scheduleChecks(ctx: Context): void {
  const updater = ctx.updater
  if (!updater || updatesDisabled()) return
  void updater.check(false)
  setInterval(() => { if (updater.due()) void updater.check(false) }, POLL_MS).unref()
}
