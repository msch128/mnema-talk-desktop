import { readBody, send } from './http.ts'
import { parseCommunityName, parseHealth } from './client.ts'
import type { ServerInfo } from './store.ts'

const MAX_DISCOVERY_BYTES = 16 * 1024

/**
 * Reads `version`, `web_client_api` and the server's own name from `/api/health` (bounded, no
 * redirects, system TLS). The answer only chooses among bundles the client
 * already verified; it never supplies code.
 */
export async function discover(origin: URL, timeoutMs = 10_000): Promise<ServerInfo> {
  let response
  try {
    response = await send({ method: 'GET', url: new URL('api/health', origin), connectTimeoutMs: timeoutMs, totalTimeoutMs: timeoutMs })
  } catch {
    throw new Error('Unable to reach the instance using trusted HTTPS.')
  }
  if (response.status !== 200) {
    response.destroy()
    throw new Error('The instance is unavailable or redirects to a different address.')
  }
  if (response.contentLength !== null && response.contentLength > MAX_DISCOVERY_BYTES) {
    response.destroy()
    throw new Error('Invalid instance response.')
  }
  const bytes = await readBody(response, MAX_DISCOVERY_BYTES)
  if (!bytes) throw new Error('Invalid instance response.')
  return { ...parseHealth(bytes), communityName: parseCommunityName(bytes) }
}
