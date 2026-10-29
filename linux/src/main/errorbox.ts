/**
 * The error dialog of the program. The native error box cannot be copied from,
 * so this one has a button that puts the full text on the clipboard.
 */
import { clipboard, dialog } from 'electron'
import { TITLE } from '../core/title.ts'

/** Shows `message` until it is dismissed with OK; "Copy" copies the message and the technical detail and keeps the dialog open. */
export async function errorBox(message: string, detail = ''): Promise<void> {
  const text = detail === '' ? message : `${message}\n${detail}`
  try {
    for (;;) {
      const { response } = await dialog.showMessageBox({ type: 'error', title: TITLE, message, detail, buttons: ['OK', 'Copy'], defaultId: 0, cancelId: 0, noLink: true })
      if (response !== 1) return
      clipboard.writeText(`${TITLE}: ${text}`)
    }
  } catch {
    // Before the app is ready only the plain box exists.
    dialog.showErrorBox(TITLE, text)
  }
}
