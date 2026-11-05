/** The answer the app origin gives a page, and the plain ones every handler shares. */

/**
 * Applied to everything that comes from the instance. `sandbox` (without
 * `allow-same-origin` or `allow-scripts`) gives a document opened from such a
 * response an opaque origin without script, whatever the server claimed.
 */
export const DATA_CSP = "sandbox; default-src 'none'"

export interface Resp {
  status: number
  headers: [string, string][]
  body: Uint8Array
}

export function plain(status: number, text: string): Resp {
  return {
    status,
    headers: [['content-type', 'text/plain; charset=utf-8'], ['x-content-type-options', 'nosniff'], ['cache-control', 'no-store']],
    body: Buffer.from(text),
  }
}

/** The server's error envelope, so the app's error handling applies. */
export function errorResp(status: number, code: string, message: string): Resp {
  return {
    status,
    headers: [['content-type', 'application/json'], ['x-content-type-options', 'nosniff'], ['cache-control', 'no-store']],
    body: Buffer.from(JSON.stringify({ error: { code, message } })),
  }
}
