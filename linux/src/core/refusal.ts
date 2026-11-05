/** Why a server cannot be served by any bundle this client holds, and what the person is told. */
import { type Version, formatVersion } from './version.ts'

/** Why no held bundle fits a server. Never answered by running the server's own JavaScript. */
export type Refusal =
  | { kind: 'no_bundle' }
  | { kind: 'server_newer' }
  | { kind: 'server_too_old', needs: Version }
  | { kind: 'server_contract_retired' }
  | { kind: 'rolled_back' }
  | { kind: 'signer_withdrawn' }

export function refusalText(refusal: Refusal): string {
  switch (refusal.kind) {
    case 'no_bundle': return 'This client has no verified app. Reinstall the desktop client.'
    case 'server_newer': return 'This server needs a newer Mnema app than this client has. Update the desktop client.'
    case 'server_too_old': return `This server is too old for this desktop client (needs ${formatVersion(refusal.needs)} or newer). Ask the administrator to update it.`
    case 'server_contract_retired': return 'This server speaks an older contract than any app in this client. Ask the administrator to update it.'
    case 'signer_withdrawn': return 'The app in this client was signed with a key that is no longer trusted. Update the desktop client; it fetches a newly signed app at its next check.'
    case 'rolled_back': return 'The newest verified app this client has used is no longer available, and older apps are refused. Update or reinstall the desktop client.'
  }
}
