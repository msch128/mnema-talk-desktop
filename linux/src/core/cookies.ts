/**
 * The instance's cookies, held by the host. The page never sees them (a port of
 * `Jar` in `experimental/app/src/web_proxy.rs`).
 *
 * The jar rests in the operating system's secret store through a `Vault`
 * (libsecret via Electron's safeStorage) where one exists; the 0600 file in the
 * application data directory is only the fallback for systems without one.
 */
import { mkdirSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { dirname } from 'node:path'

export const MAX_COOKIES = 32
/** Longest Max-Age kept, in seconds (400 days); a longer digit string would overflow to Infinity and be lost at the next restart. */
const MAX_MAX_AGE = 34_560_000

/** A place for small secrets, addressed by account name. */
export interface Vault {
  /** `null` if nothing is stored. Throws if an entry exists but cannot be read now (a locked keyring), so the caller keeps it. */
  load(account: string): Buffer | null
  /** `false` if the secret could not be stored; the caller then falls back. */
  save(account: string, secret: Buffer): boolean
  remove(account: string): void
}

interface Stored {
  name: string
  value: string
  /** Unix seconds; null for a session cookie (kept in memory only). */
  expires: number | null
}

interface Parsed {
  name: string
  value: string
  secure: boolean
  path: string | null
  domain: string | null
  maxAge: number | null
  expires: number | null
}

/** `Set-Cookie` as the `cookie` crate reads it, as far as the jar needs it. */
function parseSetCookie(raw: string): Parsed | null {
  const [pair = '', ...attributes] = raw.split(';')
  const eq = pair.indexOf('=')
  if (eq < 0) return null
  const name = pair.slice(0, eq).trim()
  if (name === '') return null
  const parsed: Parsed = { name, value: pair.slice(eq + 1).trim(), secure: false, path: null, domain: null, maxAge: null, expires: null }
  for (const attribute of attributes) {
    const at = attribute.indexOf('=')
    const key = (at < 0 ? attribute : attribute.slice(0, at)).trim().toLowerCase()
    const value = at < 0 ? null : attribute.slice(at + 1).trim()
    if (key === 'secure') parsed.secure = true
    else if (key === 'path' && value !== null && value !== '') parsed.path = value
    else if (key === 'domain' && value !== null && value.replace(/^\./, '') !== '') parsed.domain = value.replace(/^\./, '').toLowerCase()
    else if (key === 'max-age' && value !== null && /^-?[0-9]+$/.test(value)) parsed.maxAge = Math.min(MAX_MAX_AGE, Math.max(0, Number(value)))
    else if (key === 'expires' && value !== null) {
      const at2 = Date.parse(value)
      if (!Number.isNaN(at2)) parsed.expires = Math.floor(at2 / 1000)
    }
  }
  return parsed
}

export class Jar {
  private cookies: Stored[]
  private readonly file: string | null
  private readonly vault: { vault: Vault, account: string } | null

  /**
   * A jar that rests in the vault under `account`. A jar left in the file by an
   * earlier run moves into the vault and the file is removed. If the vault
   * refuses, the file keeps working.
   */
  constructor(file: string | null, vault: { vault: Vault, account: string } | null, now: number) {
    this.file = file
    this.vault = vault
    let fromVault: Stored[] | null = null
    let unreadable = false
    if (vault) {
      try {
        const raw = vault.vault.load(vault.account)
        if (raw) fromVault = parseStored(raw)
      } catch {
        // The entry exists but cannot be opened now: leave it alone, it may open later.
        unreadable = true
      }
    }
    const legacy = fromVault === null
    let cookies = fromVault
    if (cookies === null) {
      cookies = []
      if (file) {
        try { cookies = parseStored(readFileSync(file)) ?? [] } catch { cookies = [] }
      }
    }
    const live = cookies.filter((c) => c.expires !== null && c.expires > now).slice(0, MAX_COOKIES)
    this.cookies = live
    if (vault && !unreadable && (legacy || (file !== null && existsSync(file)))) this.persist(live)
  }

  private persist(cookies: Stored[]): void {
    const durable = cookies.filter((c) => c.expires !== null)
    const raw = Buffer.from(JSON.stringify(durable))
    if (this.vault) {
      if (durable.length === 0) {
        this.vault.vault.remove(this.vault.account)
        if (this.file) rmSync(this.file, { force: true })
        return
      }
      if (this.vault.vault.save(this.vault.account, raw)) {
        if (this.file) rmSync(this.file, { force: true })
        return
      }
    }
    writePrivate(this.file, raw)
  }

  /** Forgets every cookie, in memory and at rest (the secret store entry and the fallback file). Removing a server ends here. */
  forget(): void {
    this.cookies = []
    if (this.vault) this.vault.vault.remove(this.vault.account)
    if (this.file) rmSync(this.file, { force: true })
  }

  /** The `Cookie` header for a request, if any cookie applies. */
  header(now: number): string | null {
    const live = this.cookies.filter((c) => c.expires === null || c.expires > now).map((c) => `${c.name}=${c.value}`)
    return live.length > 0 ? live.join('; ') : null
  }

  /** Applies the `Set-Cookie` headers of a response from an HTTPS (or, in tests, HTTP) origin. */
  store(setCookie: readonly string[], https: boolean, now: number): void {
    let changed = false
    for (const raw of setCookie) {
      const cookie = parseSetCookie(raw)
      if (!cookie) continue
      const { name, secure } = cookie
      const prefixOk = (!name.startsWith('__Host-') || (secure && cookie.path === '/' && cookie.domain === null)) && (!name.startsWith('__Secure-') || secure)
      if (name.length > 256 || cookie.value.length > 4096 || !prefixOk || (secure && !https)) continue
      let expires: number | null = null
      if (cookie.maxAge !== null) expires = now + cookie.maxAge
      else if (cookie.expires !== null) expires = cookie.expires
      this.cookies = this.cookies.filter((c) => c.name !== name)
      changed = true
      if ((expires === null || expires > now) && this.cookies.length < MAX_COOKIES) this.cookies.push({ name, value: cookie.value, expires })
    }
    if (changed) this.persist(this.cookies)
  }
}

function parseStored(raw: Uint8Array): Stored[] | null {
  try {
    const value: unknown = JSON.parse(Buffer.from(raw).toString('utf8'))
    if (!Array.isArray(value)) return null
    const out: Stored[] = []
    for (const item of value) {
      if (typeof item !== 'object' || item === null) return null
      const { name, value: v, expires } = item as Record<string, unknown>
      if (typeof name !== 'string' || typeof v !== 'string' || !(expires === null || (typeof expires === 'number' && Number.isInteger(expires)))) return null
      out.push({ name, value: v, expires })
    }
    return out
  } catch {
    return null
  }
}

function writePrivate(file: string | null, raw: Buffer): void {
  if (!file) return
  try {
    mkdirSync(dirname(file), { recursive: true, mode: 0o700 })
    writeFileSync(file, raw, { mode: 0o600 })
  } catch { /* the session is kept in memory only */ }
}
