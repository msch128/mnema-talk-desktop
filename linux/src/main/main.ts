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
import { BaseWindow, app, dialog, protocol } from 'electron'
import { APP_SCHEME, PICKER_PREFIX, UI_PREFIX } from '../core/origin.ts'
import { passwordStoreSwitch } from '../core/osvault.ts'
import { TITLE } from '../core/title.ts'
import { Trusted, defaultDeps } from '../core/trusted.ts'
import { updatesDisabled } from '../core/updates.ts'
import { CHECK_INTERVAL_SECS } from '../core/store.ts'
import { loadAssets } from './assets.ts'
import { PARTITIONS, type Context } from './context.ts'
import { registerIpc } from './ipc.ts'
import { buildMenu } from './menu.ts'
import { HOST_PAGE_CSP, assetHandler } from './protocol.ts'
import { registerRailIpc } from './rail.ts'
import { prepare } from './sessions.ts'
import { bundlesChanged, createShell, legacyStorageWaits, wipeLegacyStorage } from './shell.ts'
import { SANDBOX_MESSAGE, debuggingAccepted, debuggingRequested, sandboxAccepted, sandboxDisabled } from './sandbox.ts'
import { createUpdater, scheduleChecks } from './updater.ts'
import { osVault } from './vault.ts'

// The scheme of the app: a secure context for WebCrypto, workers and WebRTC, with fetch and streaming for media.
protocol.registerSchemesAsPrivileged([{ scheme: APP_SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } }])

// MNEMA_DESKTOP_DATA_DIR keeps tests (and anyone who wants a separate profile) out of the usual place.
app.setPath('userData', process.env.MNEMA_DESKTOP_DATA_DIR || join(app.getPath('appData'), 'mnema-desktop'))
// Wayland where the session is Wayland (screen capture then goes through the PipeWire portal).
app.commandLine.appendSwitch('ozone-platform-hint', 'auto')
app.commandLine.appendSwitch('enable-features', 'WebRTCPipeWireCapturer')
// Window-manager-only sessions (sway, i3, Hyprland) would get Chromium's plaintext password store even with a secret service running.
const passwordStore = passwordStoreSwitch(process.env, (name) => app.commandLine.hasSwitch(name))
if (passwordStore) app.commandLine.appendSwitch('password-store', passwordStore)
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
    dialog.showErrorBox(TITLE, SANDBOX_MESSAGE)
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
  const vault = osVault(dataDir)
  if (!vault && !process.env.MNEMA_DESKTOP_NO_VAULT) console.warn('Mnema Desktop: no secret service was found, so sessions rest in a private file (mode 0600) instead of the keyring.')
  const trusted = new Trusted(dataDir, builtin, defaultDeps(vault))
  for (const name of trusted.store.rejected) {
    console.error(`Mnema Desktop: refused web bundle ${name}: it does not verify against the pinned keys.`)
  }
  const shell = { window: null, rail: null, selector: null, pages: new Map(), attachments: new Map() }
  const ctx: Context = { trusted, updater: null, debug, roles: new Map(), servers: new Map(), prepared: new Set(), shell, windows: {} }
  // The single-server client kept every server in one partition and recorded none of them: it belongs to no one server and is never handed on. Wiped once (the partition is gone afterwards).
  if (legacyStorageWaits(ctx)) void wipeLegacyStorage(ctx)

  const assets = (name: string) => loadAssets(join(app.getAppPath(), 'assets', name))
  // The rail, the page for adding a server and the pages of the servers are set up by the shell; these two are the host's own pages.
  prepare(ctx, PARTITIONS.updater, 'updater', assetHandler(UI_PREFIX, assets('update-ui'), HOST_PAGE_CSP))
  prepare(ctx, PARTITIONS.picker, 'picker', assetHandler(PICKER_PREFIX, assets('picker-ui'), HOST_PAGE_CSP))
  registerIpc(ctx)
  registerRailIpc(ctx)

  let restarting = false
  ctx.updater = createUpdater(ctx, () => { restarting = true })
  // A package kept for this start runs before any window opens.
  if (!updatesDisabled() && (await ctx.updater.applyPending()) && restarting) return
  buildMenu(ctx)
  createShell(ctx, assets)
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
    // Pages that were showing a withdrawn bundle, or none, move to the newly installed one, or show why there is none.
    if (await trusted.refresh(first)) bundlesChanged(ctx)
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
    const win = BaseWindow.getAllWindows().find((w) => w.isVisible())
    win?.show()
    win?.focus()
  })
  app.on('window-all-closed', () => app.quit())
  void app.whenReady().then(start).catch((error: unknown) => {
    console.error(`Mnema Desktop could not start: ${String(error)}`)
    app.exit(1)
  })
}
