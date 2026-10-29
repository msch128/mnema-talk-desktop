/**
 * What this build trusts. Every value is a copy of the Tauri host's, and
 * `test/pinned.test.ts` fails if the copies drift: the bundle key and the root
 * key of `experimental/app/src/web_*.rs`, the updater key and feed of
 * `experimental/app/tauri.conf.json`.
 */
import { parseVersion } from './version.ts'

/** Raw Ed25519 web bundle keys pinned in this build (rotation: pin old and new). */
export const PINNED_BUNDLE_KEYS: readonly string[] = ['7815c02fddb14038cf3641036ba2e4e20ffdd0e42d00be3477671ca4628de06d']

/** Raw Ed25519 root public key (`desktop-trust/root.pub` in mnema-talk). */
export const ROOT_PUB = '0c6b31c234adbeb1449c63acd03fcd044d7b15b640607f9943a1e3f6a819ed77'

export const TRUST_URL = 'https://raw.githubusercontent.com/msch128/mnema-talk/main/desktop-trust/trust.json'
export const TRUST_SIG_URL = 'https://raw.githubusercontent.com/msch128/mnema-talk/main/desktop-trust/trust.json.sig'
export const LATEST_RELEASE_URL = 'https://api.github.com/repos/msch128/mnema-talk/releases/latest'

/** minisign public key of the updater, in the text form of `tauri.conf.json`. */
export const UPDATER_PUBKEY = 'dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDg3MUUyOTlDQUVCRjZCMkYKUldRdmE3K3VuQ2tlaDdOajlZaG9vbENvRFBXQmtQd1pKUjZmK1hwRW8rSCt4b04rZzhwWVY3SkEK' // gitleaks:allow (a public key, the text of tauri.conf.json)
export const UPDATER_ENDPOINTS: readonly string[] = ['https://github.com/msch128/mnema-talk-desktop/releases/latest/download/latest.json']
/** The release page that deb installs point their users to. */
export const RELEASES_PAGE = 'https://github.com/msch128/mnema-talk-desktop/releases/latest'
const RELEASE_TAG_PAGE = 'https://github.com/msch128/mnema-talk-desktop/releases/tag/v'

/** The release page of a version, only for a plain `MAJOR.MINOR.PATCH`; anything else gets the latest release. */
export function releasePage(version: string): string {
  return parseVersion(version) ? `${RELEASE_TAG_PAGE}${version}` : RELEASES_PAGE
}
