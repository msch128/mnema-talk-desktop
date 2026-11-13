/**
 * Wires the self-update (`core/selfupdate.ts`) to the application: the pinned
 * key and feed, the trust statement, the AppImage that gets replaced, and the
 * restart.
 */
import { app, dialog, shell } from 'electron'
import { RELEASES_PAGE } from '../core/pinned.ts'
import { existsSync } from 'node:fs'
import { findPkexec, installDeb, isDebInstall, runCommand } from '../core/debinstall.ts'
import { UPDATER_ENDPOINTS, UPDATER_PUBKEY } from '../core/pinned.ts'
import { SelfUpdater, POLL_MS, type Snapshot, menuView, platformKey, replaceAppImage } from '../core/selfupdate.ts'
import { updatesDisabled } from '../core/updates.ts'
import type { Context } from './context.ts'
import { buildMenu } from './menu.ts'
import { openUpdater } from './windows.ts'

/** The language the rail page reported; the menu entries follow it. */
let german = false

export function menuGerman(): boolean {
  return german
}

export function setMenuGerman(ctx: Context, value: boolean): void {
  if (german === value) return
  german = value
  buildMenu(ctx)
}

/**
 * Whether a Talk runs: a server's page plays media (the voices of the others
 * in the Talk). Alone in a Talk nothing plays, and the update does not wait.
 */
function talkWatcher(ctx: Context): () => boolean {
  const playing = new Set<number>()
  app.on('web-contents-created', (_event, contents) => {
    contents.on('media-started-playing', () => { if (ctx.roles.get(contents.id) === 'instance') playing.add(contents.id) })
    contents.on('media-paused', () => { playing.delete(contents.id) })
    contents.once('destroyed', () => { playing.delete(contents.id) })
  })
  return () => playing.size > 0
}

/** Every change of the state reaches the rail's update pill. */
function pushToRail(ctx: Context, snapshot: Snapshot): void {
  const rail = ctx.shell.rail
  if (!rail || rail.webContents.isDestroyed()) return
  void rail.webContents.executeJavaScript(`window.mnemaRail&&window.mnemaRail.update&&window.mnemaRail.update(${JSON.stringify(snapshot)})`).catch(() => undefined)
}

/** The AppImage this process was started from; set by the AppImage runtime. */
function appImagePath(): string | null {
  const path = process.env.APPIMAGE
  return path && path.length > 0 ? path : null
}

/** Installed from the `.deb`: the package manager installs the verified update. */
function debInstalled(): boolean {
  return isDebInstall(process.execPath, process.env)
}

export function createUpdater(ctx: Context, onRestart: () => void): SelfUpdater {
  const image = appImagePath()
  const deb = image === null && debInstalled()
  let shown = ''
  const inTalk = talkWatcher(ctx)
  return new SelfUpdater({
    current: app.getVersion(),
    dir: app.getPath('userData'),
    endpoints: UPDATER_ENDPOINTS.map((e) => new URL(e)),
    pubkey: UPDATER_PUBKEY,
    platform: platformKey(process.platform, process.arch, deb),
    statement: () => ctx.trusted.statement(),
    installsAllowed: () => ctx.trusted.fresh(),
    trustRound: () => ctx.trusted.trustRound(),
    install: (bytes) => {
      if (deb) {
        return installDeb(bytes, {
          run: runCommand,
          hasPkexec: () => findPkexec(existsSync),
          open: async (path) => { const error = await shell.openPath(path); if (error) throw new Error('could not open the package') },
        }).then((result) => (result === 'handoff' ? 'handoff' as const : undefined))
      }
      if (!image) return Promise.reject(new Error('not an AppImage'))
      replaceAppImage(image, bytes)
      return Promise.resolve()
    },
    confirmStart: deb
      ? async (version) => (await dialog.showMessageBox({
        type: 'question',
        title: 'Mnema Desktop update',
        message: `Install Mnema Desktop ${version} now?`,
        detail: 'The update was verified. Installing it needs your password, then Mnema Desktop starts again. Choose "Later" to start the current version and be asked again next time.',
        buttons: ['Install now', 'Later'],
        defaultId: 0,
        cancelId: 1,
      })).response === 0
      : undefined,
    restart: () => {
      onRestart()
      if (image) app.relaunch({ execPath: image, args: process.argv.slice(1) })
      else app.relaunch()
      app.exit(0)
    },
    now: () => Math.floor(Date.now() / 1000),
    githubOnly: true,
    external: image === null && !deb,
    onChange: (snapshot) => {
      // The menu says only whether something waits; progress does not rebuild it.
      const view = JSON.stringify(menuView(snapshot, german))
      if (view !== shown) { shown = view; buildMenu(ctx) }
      pushToRail(ctx, snapshot)
    },
    // A background check opens no window: the rail's pill and the menu show what it found.
    inTalk,
  })
}

/**
 * "Update now" from the rail, the menu or the window. `confirmed` answers the
 * question about a running Talk. An installation of neither kind only gets the
 * release page; whatever needs a question or has nothing to install opens the window.
 */
export function updateNow(ctx: Context, confirmed: boolean): void {
  const updater = ctx.updater
  if (!updater) return
  if (updater.snapshot().external) { void shell.openExternal(RELEASES_PAGE); return }
  const choice = updater.requestNow(confirmed)
  if (choice === 'ask_about_talk' || choice === 'show') openUpdater(ctx, true)
  if (choice === 'show' && !updater.isBusyOrWaiting()) void updater.check(true)
}

/** A check at every start, then whenever the interval has passed. */
export function scheduleChecks(ctx: Context): void {
  const updater = ctx.updater
  if (!updater || updatesDisabled()) return
  void updater.check(false)
  setInterval(() => { if (updater.due()) void updater.check(false) }, POLL_MS).unref()
}
