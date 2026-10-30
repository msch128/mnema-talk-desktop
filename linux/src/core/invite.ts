/**
 * What to do with a link a server's page opens outside the app when it is an
 * invite to a server: offer to add that server, open the join screen of one
 * that is already here, or leave it to the browser as any other link. Free of
 * Electron so the decision can be tested.
 */
import { inviteCode, parseAddress } from './client.ts'
import { idFor } from './instances.ts'

export type InviteChoice = 'add' | 'known' | 'external'

/**
 * Only an `https://<host>/invite/<code>` link without credentials counts. The
 * server it names is taken exactly as the address would be typed in by hand;
 * `known` says whether that server is in the list already, `room` whether
 * another one fits.
 */
export function inviteChoice(url: URL, known: (id: string) => boolean, room: boolean): InviteChoice {
  if (url.protocol !== 'https:' || url.username !== '' || url.password !== '' || !url.pathname.startsWith('/invite/') || inviteCode(url.href) === null) return 'external'
  let id: string
  try { id = idFor(parseAddress(url.href)) } catch { return 'external' }
  if (known(id)) return 'known'
  return room ? 'add' : 'external'
}
