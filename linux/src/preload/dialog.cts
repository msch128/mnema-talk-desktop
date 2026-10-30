/**
 * The preload of the rail's dialog window, a page of the host. It hands the
 * page what the dialog is to show (`spec`, given by the host before any script
 * runs) and one way to answer (`done`). The main process answers only this
 * window, once, and checks the answer against the spec. The page gets no other
 * command and no Tauri bridge.
 *
 * A single file with no imports of its own: a sandboxed preload cannot load
 * local modules.
 */
// eslint-disable-next-line @typescript-eslint/no-require-imports -- a sandboxed preload may only require 'electron'
const { contextBridge, ipcRenderer } = require('electron') as typeof import('electron')

interface Answer { button: string, value: string | null, color: string | null }

const spec: unknown = JSON.parse(String(ipcRenderer.sendSync('mnema:dialog-spec')))

function done(answer: Answer): void {
  const clean: Answer = {
    button: String(answer.button),
    value: typeof answer.value === 'string' ? answer.value : null,
    color: typeof answer.color === 'string' ? answer.color : null,
  }
  ipcRenderer.send('mnema:dialog-done', clean)
}

contextBridge.exposeInMainWorld('__MNEMA_DIALOG_HOST__', { spec, done })
