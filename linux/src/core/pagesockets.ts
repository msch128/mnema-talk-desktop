/**
 * The sockets a page holds end with the page. The host relays a socket's events
 * to the web contents, and a document that replaces the previous one in the
 * same contents (a reload, `location.assign`) has no callback for them: left
 * open they would count against the budget and keep the user online for
 * nothing.
 */
import type { Sockets } from './sockets.ts'

/** The part of a web contents this needs. */
export interface PageEvents {
  on(event: 'did-start-navigation', listener: (details: { isMainFrame: boolean, isSameDocument: boolean }) => void): unknown
  on(event: 'render-process-gone', listener: () => void): unknown
}

/** Closes the sockets when the page leaves its document or its renderer dies. `sockets` is looked up on every event: the server may have a new session by then. */
export function closeSocketsWithDocument(page: PageEvents, sockets: () => Pick<Sockets, 'closeAll'> | undefined): void {
  page.on('did-start-navigation', (details) => {
    if (details.isMainFrame && !details.isSameDocument) sockets()?.closeAll()
  })
  page.on('render-process-gone', () => sockets()?.closeAll())
}
