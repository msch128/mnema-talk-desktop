import { readBody, send } from './http.ts'
import { parseCommunityName, parseDegradedHealth, parseHealth } from './client.ts'
import { iconWanted } from './icon.ts'
import type { ServerInfo } from './store.ts'

const MAX_DISCOVERY_BYTES = 16 * 1024
export const UNAVAILABLE = 'The instance is unavailable or redirects to a different address.'
export const INVALID_RESPONSE = 'Invalid instance response.'

export const NAT_HINT = 'If the server runs in your own network, your router may not support reaching it through its public address (NAT loopback/hairpin) — try the local address or another network.'
const CERTIFICATE_CODE = /^(CERT_|ERR_TLS_CERT|UNABLE_TO_|DEPTH_ZERO_|SELF_SIGNED_)/

/**
 * Why the instance could not be reached, in words the person can act on. A
 * certificate refusal keeps its OpenSSL code so a report shows the real cause.
 */
export function unreachableReason(error: unknown): string {
  const code = error instanceof Error ? String((error as NodeJS.ErrnoException).code ?? '') : ''
  const name = error instanceof Error ? error.name : ''
  if (code === 'ECONNREFUSED') return `The instance refused the connection. Check the address and the port. ${NAT_HINT}`
  if (code === 'ENOTFOUND' || code === 'EAI_AGAIN') return 'The instance address could not be resolved. Check the address and your network.'
  // The request's own deadline reaches us as an AbortError that carries the TimeoutError of `AbortSignal.timeout`.
  const cause = error instanceof Error ? error.cause : undefined
  const deadline = name === 'TimeoutError' || (cause instanceof Error && cause.name === 'TimeoutError')
  if (code === 'ETIMEDOUT' || deadline || (error instanceof Error && error.message === 'connect timeout')) {
    return `The instance did not answer in time. Check the address, the port and your network. ${NAT_HINT}`
  }
  if (CERTIFICATE_CODE.test(code)) {
    return `This computer does not trust the instance's HTTPS certificate. Check the system clock, and that no antivirus or network filter intercepts HTTPS. (${code})`
  }
  return 'Unable to reach the instance using trusted HTTPS.'
}

/**
 * Reads `version`, `web_client_api` and the server's own name from `/api/health` (bounded, no
 * redirects, system TLS). The answer only chooses among bundles the client
 * already verified; it never supplies code. A `503` counts only when its body
 * says what the server is (`parseDegradedHealth`); every other status is no answer.
 */
export async function discover(origin: URL, timeoutMs = 10_000): Promise<ServerInfo> {
  let response
  try {
    response = await send({ method: 'GET', url: new URL('api/health', origin), connectTimeoutMs: timeoutMs, totalTimeoutMs: timeoutMs })
  } catch (error) {
    throw new Error(unreachableReason(error), { cause: error })
  }
  const degraded = response.status === 503
  if (response.status !== 200 && !degraded) {
    response.destroy()
    throw new Error(UNAVAILABLE)
  }
  if (response.contentLength !== null && response.contentLength > MAX_DISCOVERY_BYTES) {
    response.destroy()
    throw new Error(degraded ? UNAVAILABLE : INVALID_RESPONSE)
  }
  const bytes = await readBody(response, MAX_DISCOVERY_BYTES)
  if (!bytes) throw new Error(degraded ? UNAVAILABLE : INVALID_RESPONSE)
  const server = degraded ? parseDegradedHealth(bytes) : parseHealth(bytes)
  if (!server) throw new Error(UNAVAILABLE)
  return { ...server, communityName: parseCommunityName(bytes), communityIcon: iconWanted(bytes) }
}
