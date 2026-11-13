/**
 * What the error dialog says about a download that saved nothing. The same
 * table is in the Tauri host (`src/web_download/text.rs`); `download_errors`
 * in `test-vectors/proxy.json` keeps the two equal. `{message}`, `{status}`
 * and `{reason}` are filled in.
 */
export const SENTENCES = {
  bad_path: 'This link cannot be downloaded.',
  unreachable: 'The server could not be reached, so nothing was saved. Check the connection and try again.',
  gone: 'There is nothing to download: the file was downloaded already, removed or has expired. For your data export, request a new one under Settings.',
  refused: 'The server refused the download: {message}. Try again later.',
  failed: 'The download failed (HTTP {status}). Try again later.',
  too_large: 'The file is larger than this client accepts, so nothing was saved.',
  cut: 'The download was interrupted, so nothing was saved. Try again.',
  disk: 'The file could not be saved: {reason}. Check the folder, then try again.',
  busy: 'Another download is still in progress. Try again when it has finished.',
  no_folder: 'There is no Downloads folder to save the file in. Create one, then try again.',
  webview_failed: 'The download failed, so nothing was saved. Try again.',
} as const

/** Why a file could not be written. */
export const REASONS = {
  no_space: 'there is no space left on the disk',
  denied: 'this app may not write to the folder',
  read_only: 'the disk is read-only',
  missing: 'the folder does not exist',
  name_too_long: 'the name is too long for this disk',
  quota: 'the disk quota is used up',
  other: 'the disk reported an error',
} as const

export type Reason = keyof typeof REASONS

/** The reason for a Node error code. */
export function reasonOf(code: unknown): Reason {
  switch (code) {
    case 'ENOSPC': return 'no_space'
    case 'EACCES': case 'EPERM': return 'denied'
    case 'EROFS': return 'read_only'
    case 'ENOENT': case 'ENOTDIR': return 'missing'
    case 'ENAMETOOLONG': return 'name_too_long'
    case 'EDQUOT': return 'quota'
    default: return 'other'
  }
}

export const diskSentence = (reason: Reason): string => SENTENCES.disk.replace('{reason}', REASONS[reason])

/** Longest message of the server that is passed on, in characters. */
const MAX_MESSAGE_CHARS = 300

const isControl = (c: string): boolean => {
  const code = c.codePointAt(0) as number
  return code < 0x20 || (code >= 0x7f && code <= 0x9f)
}

/**
 * A server message as the person may see it: control characters become
 * spaces, runs of spaces one, no surrounding spaces or final full stops, at most 300 characters; null when
 * nothing is left. Blanks are spaces here, as in the Rust host.
 */
export function cleanMessage(message: string): string | null {
  const kept = [...message].map((c) => (isControl(c) ? ' ' : c)).join('').replace(/ {2,}/g, ' ')
  const cut = [...kept.replace(/^ +| +$/g, '')].slice(0, MAX_MESSAGE_CHARS).join('').replace(/[ .]+$/, '')
  return cut === '' ? null : cut
}

/** What a person is told of a refusal: the 404 of a spent file, the server's own message, or the status. */
export function refusalMessage(status: number, message: string | null): string {
  if (status === 404) return SENTENCES.gone
  if (message !== null) return SENTENCES.refused.replace('{message}', message)
  return SENTENCES.failed.replace('{status}', String(status))
}
