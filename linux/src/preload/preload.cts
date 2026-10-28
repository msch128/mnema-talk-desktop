/**
 * The only script that runs next to the page (sandboxed, context isolated, no
 * Node). The web app is built for the Tauri host: it reads
 * `window.__TAURI_INTERNALS__` and `window.isTauri` for its IPC (`invoke`,
 * `Channel`) and `window.__MNEMA_TRUSTED_APP__` for the identity of the
 * verified bundle. This script offers exactly that surface, over one IPC
 * channel to the main process, which decides per window what is allowed. The
 * page never sees `ipcRenderer`, and no other command exists.
 *
 * It is a single file with no imports of its own: a sandboxed preload cannot
 * load local modules.
 */
// eslint-disable-next-line @typescript-eslint/no-require-imports -- a sandboxed preload may only require 'electron'
const { contextBridge, ipcRenderer } = require('electron') as typeof import('electron')

interface Reply { ok: true, value: unknown }
interface Failure { ok: false, error: string }

/** Callbacks the page registered (`transformCallback`), by the id handed back to it. */
const callbacks = new Map<number, { callback: (payload: unknown) => void, once: boolean }>()
let nextCallback = 1

ipcRenderer.on('mnema:callback', (_event, id: unknown, payload: unknown) => {
  if (typeof id !== 'number') return
  const entry = callbacks.get(id)
  if (!entry) return
  if (entry.once) callbacks.delete(id)
  try {
    entry.callback(payload)
  } catch {
    // A throwing page handler must not stop later messages.
  }
})

/** A channel the page passed in arrives as a plain object with its numeric id. */
function channelId(value: unknown): number | null {
  if (typeof value === 'object' && value !== null) {
    const id: unknown = Reflect.get(value, 'id')
    if (typeof id === 'number' && Number.isSafeInteger(id)) return id
  }
  return null
}

async function invoke(command: unknown, args?: unknown): Promise<unknown> {
  if (typeof command !== 'string' || command.length > 64) throw new Error('invalid command')
  const input: Record<string, unknown> = {}
  if (typeof args === 'object' && args !== null) {
    for (const key of Object.keys(args)) {
      const value: unknown = Reflect.get(args, key)
      if (key === 'onEvent') input.onEvent = channelId(value)
      else if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' || value === null) input[key] = value
    }
  }
  const reply = (await ipcRenderer.invoke('mnema:invoke', command, input)) as Reply | Failure
  if (reply.ok) return reply.value
  throw reply.error
}

contextBridge.exposeInMainWorld('isTauri', true)
contextBridge.exposeInMainWorld('__TAURI_INTERNALS__', {
  invoke,
  transformCallback(callback: unknown, once?: unknown): number {
    if (typeof callback !== 'function') throw new Error('invalid callback')
    const id = nextCallback++
    callbacks.set(id, { callback: callback as (payload: unknown) => void, once: once === true })
    return id
  },
  unregisterCallback(id: unknown): void {
    if (typeof id === 'number') callbacks.delete(id)
  },
  convertFileSrc(_path: unknown): string {
    // Nothing is served from the file system.
    return ''
  },
})

// The identity of the verified bundle this window runs, set before any page
// script (only the top frame of the app's own pages receives one).
const identity: unknown = ipcRenderer.sendSync('mnema:identity')
if (identity !== null && typeof identity === 'object') {
  contextBridge.exposeInMainWorld('__MNEMA_TRUSTED_APP__', identity)
}
