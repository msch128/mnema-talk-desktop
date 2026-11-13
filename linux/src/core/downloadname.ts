/**
 * The name a download is saved under. The server names it (`Content-Disposition`),
 * the page may suggest one (the `download` attribute), the address is the last
 * resort; whatever is chosen is cut down to a plain file name, so no answer can
 * point the file at another folder. The cases are in `download_names` of `test-vectors/proxy.json`,
 * which the Rust host reads as well.
 */

function isControl(code: number): boolean {
  return code < 0x20 || (code >= 0x7f && code <= 0x9f)
}

/**
 * The parameters of a `Content-Disposition` value by lower-case name, the first
 * of each. A value is a quoted string, taken as written (no escapes), or a
 * token up to the next `;`.
 */
function parameters(disposition: string): Map<string, string> {
  const found = new Map<string, string>()
  const start = disposition.indexOf(';')
  if (start < 0) return found
  for (const match of disposition.slice(start).matchAll(/;\s*([^=;]*?)\s*=\s*(?:"([^"]*)"|([^;]*))/g)) {
    const name = (match[1] as string).toLowerCase()
    if (!found.has(name)) found.set(name, match[2] ?? (match[3] as string).trim())
  }
  return found
}

/** `filename*` (RFC 5987): `UTF-8''` and percent-encoded bytes; any other charset or broken encoding is ignored. */
function extended(value: string): string | null {
  const match = /^utf-8'[^']*'(.*)$/i.exec(value)
  if (!match) return null
  const bytes: number[] = []
  const text = match[1] as string
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '%') {
      const hex = text.slice(i + 1, i + 3)
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return null
      bytes.push(parseInt(hex, 16))
      i += 2
    } else {
      bytes.push(...Buffer.from(text[i] as string))
    }
  }
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(bytes))
  } catch {
    return null
  }
}

/** The last segment of the path of an address, without query and fragment, percent-decoded where it can be. */
function lastSegment(path: string): string {
  const bare = path.split(/[?#]/)[0] as string
  const segment = bare.slice(bare.lastIndexOf('/') + 1)
  try {
    return decodeURIComponent(segment)
  } catch {
    return segment
  }
}

/** A plain file name: what follows the last `/` or `\`, no control characters, trimmed, no leading dots; "download" if nothing is left. */
export function plainName(raw: string): string {
  const cut = Math.max(raw.lastIndexOf('/'), raw.lastIndexOf('\\'))
  const kept = [...raw.slice(cut + 1)].filter((c) => !isControl(c.codePointAt(0) as number)).join('').trim()
  const name = kept.replace(/^\.+/, '')
  return name === '' ? 'download' : name
}

/**
 * The name for a download: `filename*` of the disposition, else `filename`,
 * else the page's `download` attribute, else the last segment of `path`.
 * A candidate that is empty is skipped.
 */
export function downloadName(disposition: string | null, attribute: string, path: string): string {
  const params = disposition === null ? new Map<string, string>() : parameters(disposition)
  const star = params.get('filename*')
  const candidates = [star === undefined ? null : extended(star), params.get('filename') ?? null, attribute, lastSegment(path)]
  return plainName(candidates.find((c) => c !== null && c.trim() !== '') ?? '')
}
