import { readBody, send } from './http.ts'
import { parseHealth } from './client.ts'
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
  if (code === 'ETIMEDOUT' || name === 'TimeoutError' || (error instanceof Error && error.message === 'connect timeout')) {
    return `The instance did not answer in time. Check the address, the port and your network. ${NAT_HINT}`
  }
  if (CERTIFICATE_CODE.test(code)) {
    return `This computer does not trust the instance's HTTPS certificate. Check the system clock, and that no antivirus or network filter intercepts HTTPS. (${code})`
  }
  return 'Unable to reach the instance using trusted HTTPS.'
}

/**
 * Reads `version` and `web_client_api` from `/api/health` (bounded, no
 * redirects, system TLS). The answer only chooses among bundles the client
 * already verified; it never supplies code.
 */
export async function discover(origin: URL, timeoutMs = 10_000): Promise<ServerInfo> {
  let response
  try {
    response = await send({ method: 'GET', url: new URL('api/health', origin), connectTimeoutMs: timeoutMs, totalTimeoutMs: timeoutMs })
  } catch (error) {
    throw new Error(unreachableReason(error), { cause: error })
  }
  if (response.status !== 200) {
    response.destroy()
    throw new Error(UNAVAILABLE)
  }
  if (response.contentLength !== null && response.contentLength > MAX_DISCOVERY_BYTES) {
    response.destroy()
    throw new Error(INVALID_RESPONSE)
  }
  const bytes = await readBody(response, MAX_DISCOVERY_BYTES)
  if (!bytes) throw new Error(INVALID_RESPONSE)
  return parseHealth(bytes)
}
