/**
 * Reading the release feed for the self-update (`selfupdate.ts`): the first
 * endpoint that answers, this machine's entry, and the key the trust statement
 * allows for the announced signature.
 */
import { chooseUpdaterKey } from './trust.ts'
import { fetchLimited } from './updates.ts'
import { type Update, downloadUrlOk, offerable, parseFeed } from './updatefeed.ts'
import { type UpdaterEnv, FailureError } from './updatertypes.ts'

export const CHECK_TIMEOUT_MS = 30_000
export const DOWNLOAD_TIMEOUT_MS = 15 * 60 * 1000
export const START_TIMEOUT_MS = 8_000
const MAX_FEED = 1 << 20
export const MAX_PACKAGE = 512 << 20

/** The newer release this machine may install, or null. */
export async function fetchUpdate(env: UpdaterEnv): Promise<Update | null> {
  const platform = env.platform
  if (platform === null) return null
  let feedBytes: Buffer | null = null
  let failure: unknown = null
  for (const endpoint of env.endpoints) {
    try {
      feedBytes = await fetchLimited(endpoint, 'application/json', MAX_FEED, { ...env.fetchOptions, timeoutMs: CHECK_TIMEOUT_MS })
      break
    } catch (error) {
      failure = error
    }
  }
  if (!feedBytes) throw failure ?? new FailureError('offline')
  const found = parseFeed(feedBytes, platform)
  if (!found || offerable(env.current, found.version) === null) return null
  // The trust statement decides which updater key an update may be signed
  // with: a revoked key is refused, a listed replacement key is used.
  const chosen = chooseUpdaterKey(env.pubkey, env.statement(), found.signature)
  if (!chosen.ok) throw new FailureError('verify')
  if (env.githubOnly && !downloadUrlOk(found.url)) return null
  return { ...found, key: chosen.key }
}
