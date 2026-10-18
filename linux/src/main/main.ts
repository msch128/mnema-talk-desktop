/**
 * Mnema Desktop for Linux (Electron). The web app runs from a signed bundle
 * that this process verified, at the origin `mnema://localhost`; the selected
 * instance is reached through this process (`core/proxy.ts`, `core/sockets.ts`),
 * so it supplies data and never code. See mnema-talk
 * `doc/desktop-trusted-client.md`. The Tauri host does the same on Windows and
 * macOS; the verification code is a port of it, checked against shared vectors.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import tls from 'node:tls'
import { BrowserWindow, app, dialog, protocol, session } from 'electron'
import { APP_SCHEME, PICKER_PREFIX, UI_PREFIX, APP_URL } from '../core/origin.ts'
import { Trusted, defaultDeps } from '../core/trusted.ts'
import { checkAndInstall, updatesDisabled } from '../core/updates.ts'
import { CHECK_INTERVAL_SECS } from '../core/store.ts'
import { loadAssets } from './assets.ts'
import { PARTITIONS, type Context } from './context.ts'
import { registerIpc } from './ipc.ts'
import { buildMenu } from './menu.ts'
import { secureSession } from './permissions.ts'
import { HOST_PAGE_CSP, appHandler, assetHandler } from './protocol.ts'
import { SANDBOX_MESSAGE, debuggingAccepted, debuggingRequested, sandboxAccepted, sandboxDisabled } from './sandbox.ts'
import { createUpdater, scheduleChecks } from './updater.ts'
import { osVault } from './vault.ts'
import { openSelector } from './windows.ts'

// The scheme of the app: a secure context for WebCrypto, workers and WebRTC, with fetch and streaming for media.
protocol.registerSchemesAsPrivileged([{ scheme: APP_SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } }])

// MNEMA_DESKTOP_DATA_DIR keeps tests (and anyone who wants a separate profile) out of the usual place.
app.setPath('userData', process.env.MNEMA_DESKTOP_DATA_DIR || join(app.getPath('appData'), 'mnema-desktop'))
// Wayland where the session is Wayland (screen capture then goes through the PipeWire portal).
app.commandLine.appendSwitch('ozone-platform-hint', 'auto')
app.commandLine.appendSwitch('enable-features', 'WebRTCPipeWireCapturer')
// No app.enableSandbox(): it also removes --no-sandbox from the command line, which the start check below must see.
// Every window sets sandbox: true itself.

const debug = process.env.MNEMA_DEBUG_PROTOCOL !== undefined && process.env.MNEMA_DEBUG_PROTOCOL !== ''

/** Certificates are checked against the system store, as the Tauri host's native TLS does (a community may run on a private CA). */
function useSystemCertificates(): void {
  try {
    const system = tls.getCACertificates('system')
    if (system.length > 0) tls.setDefaultCACertificates(system)
  } catch { /* the bundled roots stay in force */ }
}

async function start(): Promise<void> {
  if (sandboxDisabled((name) => app.commandLine.hasSwitch(name), process.env) && !sandboxAccepted(process.env)) {
    console.error(SANDBOX_MESSAGE)
    dialog.showErrorBox('Mnema Desktop DEV', SANDBOX_MESSAGE)
    app.exit(1)
    return
  }
  if (app.isPackaged && debuggingRequested((name) => app.commandLine.hasSwitch(name)) && !debuggingAccepted(process.env)) {
    console.error('Mnema Desktop does not open a remote debugging port. Set MNEMA_DESKTOP_ALLOW_REMOTE_DEBUGGING=1 to allow it.')
    app.exit(1)
    return
  }
  useSystemCertificates()
  const dataDir = app.getPath('userData')
  let builtin: Buffer | null = null
  try {
    builtin = readFileSync(join(app.getAppPath(), 'bundle', 'builtin.zip'))
    if (builtin.length === 0) builtin = null
  } catch { /* a build without a bundle has to download one */ }
  const trusted = new Trusted(dataDir, builtin, defaultDeps(osVault(dataDir)))
  for (const name of trusted.store.rejected) {
    console.error(`Mnema Desktop: refused web bundle ${name}: it does not verify against the pinned keys.`)
  }
  const ctx: Context = { trusted, updater: null, debug, roles: new Map(), windows: {} }

  const assets = (name: string) => loadAssets(join(app.getAppPath(), 'assets', name))
  const handlers = {
    selector: appHandler('selector', trusted, debug),
    instance: appHandler('instance', trusted, debug),
    attachment: appHandler('attachment', trusted, debug),
    updater: assetHandler(UI_PREFIX, assets('update-ui'), HOST_PAGE_CSP),
    picker: assetHandler(PICKER_PREFIX, assets('picker-ui'), HOST_PAGE_CSP),
  }
  for (const role of Object.keys(handlers) as (keyof typeof handlers)[]) {
    const ses = session.fromPartition(PARTITIONS[role])
    ses.protocol.handle(APP_SCHEME, handlers[role])
    secureSession(ctx, ses, role)
  }
  registerIpc(ctx)

  let restarting = false
  ctx.updater = createUpdater(ctx, () => { restarting = true })
  // A package kept for this start runs before any window opens.
  if (!updatesDisabled() && (await ctx.updater.applyPending()) && restarting) return
  buildMenu(ctx)
  openSelector(ctx)
  scheduleChecks(ctx)
  refreshInBackground(ctx)
}

/**
 * Verifies updates in the background: the trust statement first (a revoked key
 * must stop counting before anything is installed), then newer bundles. A
 * bundle that arrives while none is usable (first start without a built-in one)
 * is picked up at once.
 */
function refreshInBackground(ctx: Context): void {
  const { trusted } = ctx
  let first = true
  const once = async () => {
    if (updatesDisabled()) return
    const dropped = await trusted.trustRound()
    const hadBundle = trusted.store.newest() !== undefined
    const now = Math.floor(Date.now() / 1000)
    const due = trusted.store.checkDue(now)
    if (hadBundle && !dropped && !(due || !first)) return
    let installed = null
    // Freeze protection: no new bundles without a fresh trust statement.
    if (trusted.fresh()) {
      installed = await checkAndInstall(trusted.store, () => trusted.keysInForce()).catch(() => null)
    }
    trusted.store.markChecked(Math.floor(Date.now() / 1000))
    // A window that was showing a withdrawn bundle, or none, moves to the newly installed one, or shows why there is none.
    const selector = ctx.windows.selector
    if ((dropped || (!hadBundle && installed !== null)) && selector && !selector.isDestroyed()) void selector.loadURL(APP_URL)
  }
  const loop = async () => {
    await once().catch((error: unknown) => console.error(`Mnema Desktop: update round failed: ${String(error)}`))
    first = false
  }
  void loop()
  setInterval(() => { void loop() }, CHECK_INTERVAL_SECS * 1000).unref()
}

if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', () => {
    const win = BrowserWindow.getAllWindows().find((w) => w.isVisible())
    win?.show()
    win?.focus()
  })
  app.on('window-all-closed', () => app.quit())
  void app.whenReady().then(start).catch((error: unknown) => {
    console.error(`Mnema Desktop could not start: ${String(error)}`)
    app.exit(1)
  })
}
