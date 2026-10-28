/** The part of an Electron download item that `closeWhenStarted` uses. */
export interface DownloadEvents {
  once(event: 'updated' | 'done', listener: () => void): unknown
}

/**
 * An attachment the proxy answers with a download never shows a document, so
 * its window is closed as soon as the download is under way (the save dialog is
 * over by then) or has ended.
 */
export function closeWhenStarted(item: DownloadEvents, close: () => void): void {
  let closed = false
  const once = () => {
    if (closed) return
    closed = true
    close()
  }
  item.once('updated', once)
  item.once('done', once)
}
