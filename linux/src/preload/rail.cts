/**
 * The preload of the server rail, a page of the host. It offers the rail page
 * one function, `invoke`, over its own IPC channel; the main process answers
 * only calls that come from the rail's own page. The pages of the servers never
 * get this script, and the rail never gets the Tauri bridge.
 *
 * A single file with no imports of its own: a sandboxed preload cannot load
 * local modules.
 */
// eslint-disable-next-line @typescript-eslint/no-require-imports -- a sandboxed preload may only require 'electron'
const { contextBridge, ipcRenderer } = require('electron') as typeof import('electron')

interface Reply { ok: true, value: unknown }
interface Failure { ok: false, error: string }

async function invoke(command: unknown, args?: unknown): Promise<unknown> {
  if (typeof command !== 'string' || command.length > 64) throw new Error('invalid command')
  const input: Record<string, unknown> = {}
  if (typeof args === 'object' && args !== null) {
    for (const key of Object.keys(args)) {
      const value: unknown = Reflect.get(args, key)
      if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' || value === null) input[key] = value
    }
  }
  const reply = (await ipcRenderer.invoke('mnema:rail', command, input)) as Reply | Failure
  if (reply.ok) return reply.value
  throw reply.error
}

contextBridge.exposeInMainWorld('__MNEMA_RAIL_HOST__', { invoke })
