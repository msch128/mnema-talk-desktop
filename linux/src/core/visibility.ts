/**
 * Which pages of the shell are shown. A minimised window shows none, so the
 * engine treats every page as a background page (`document.visibilityState` is
 * `hidden`, no frames are produced). Hidden pages are throttled, never
 * suspended: their sockets feed the rail's dots and the toasts, and the audio
 * of a call goes on.
 */
export function pageShown(id: string | null, front: string | null, minimized: boolean): boolean {
  return !minimized && id === front
}

/** The rail is shown whenever the window is. */
export function railShown(minimized: boolean): boolean {
  return !minimized
}
