/**
 * What the proxy lets through: body caps that follow the upload limit the
 * instance announces, and a memory budget shared by all large transfers.
 */

/**
 * Largest body the proxy buffers before the instance has announced its upload
 * limit (`max_upload_mb` of `GET /api/permissions`). Every byte is in memory;
 * the caps keep one request from taking the machine's RAM. Audio and video
 * arrive in `Range` pieces and are not affected.
 */
export const MAX_BODY = 128 << 20
/** What the server's JSON routes accept (1 MiB), with headroom. */
const MAX_JSON_BODY = 2 << 20
/** Multipart framing and form fields around an uploaded file. */
export const MULTIPART_OVERHEAD = 1 << 20
/** Avatars are read in memory by the server and capped at 5 MiB. */
const MAX_AVATAR_BODY = (5 << 20) + MULTIPART_OVERHEAD
/** The server's own ceiling (`MAX_UPLOAD_SIZE_MB` is at most 2048). */
const MAX_ANNOUNCED_MB = 2048
/** Bodies up to this size are handled without asking the memory budget. */
export const SMALL_BODY = 32 << 20
/** Memory the proxy lends to bodies above `SMALL_BODY`, all transfers together. */
export const LARGE_BODY_BUDGET = 1 << 30
const BUDGET_UNIT = 64 << 10
const BUDGET_WAIT_MS = 30_000

// ----------------------------------------------------------------- limits

class Budget {
  private free: number
  private readonly capacity: number
  private readonly waiting: { units: number, grant: () => void }[] = []

  constructor(capacity: number) {
    this.capacity = capacity
    this.free = capacity
  }

  /** Waits for `units`; resolves to a release function, or null after `timeoutMs`. */
  acquire(units: number, timeoutMs: number): Promise<(() => void) | null> {
    if (units > this.capacity) return Promise.resolve(null)
    return new Promise((resolve) => {
      const release = () => { this.free += units; this.drain() }
      const waiter = { units, grant: () => { clearTimeout(timer); this.free -= units; resolve(release) } }
      const timer = setTimeout(() => {
        const at = this.waiting.indexOf(waiter)
        if (at >= 0) this.waiting.splice(at, 1)
        resolve(null)
        // A head that gave up may have been the only thing keeping smaller requests behind it from the free memory.
        this.drain()
      }, timeoutMs)
      this.waiting.push(waiter)
      this.drain()
    })
  }

  private drain(): void {
    // First come, first served: a large request is not overtaken forever.
    while (this.waiting.length > 0 && (this.waiting[0] as { units: number }).units <= this.free) {
      (this.waiting.shift() as { grant: () => void }).grant()
    }
  }
}

/**
 * Routes that take one picture, parsed in memory by the server: its member
 * avatar and banner, the community icon, the avatar of a bot or webhook, the
 * cover of an event (each with an id between the fixed parts) and a picture of
 * the wiki.
 */
const PICTURE_ROUTES = [
  /^\/api\/users\/me\/(?:avatar|banner)$/,
  /^\/api\/admin\/community\/icon$/,
  /^\/api\/admin\/(?:bots|webhooks)\/[^/]+\/avatar$/,
  /^\/api\/events\/[^/]+\/cover$/,
  /^\/api\/wiki\/images$/,
]

function routeKind(path: string): 'picture' | 'upload' | 'json' {
  if (PICTURE_ROUTES.some(route => route.test(path))) return 'picture'
  const rest = path.startsWith('/api/channels/') ? path.slice('/api/channels/'.length) : null
  const suffix = rest?.endsWith('/upload') ? '/upload' : rest?.endsWith('/attachments') ? '/attachments' : null
  const id = rest !== null && suffix !== null ? rest.slice(0, -suffix.length) : null
  return id !== null && id !== '' && !id.includes('/') ? 'upload' : 'json'
}

/** What the proxy lets through, following what the instance announces. */
export class Limits {
  /** The effective upload limit of the signed-in member in MB; 0 until the instance has announced one. */
  private uploadMb = 0
  private readonly budget: Budget
  private readonly waitMs: number

  constructor(waitMs = BUDGET_WAIT_MS, capacity = LARGE_BODY_BUDGET) {
    this.waitMs = waitMs
    this.budget = new Budget(Math.floor(capacity / BUDGET_UNIT))
  }

  /** Takes the instance's announcement; nonsense is ignored. */
  announceUploadMb(mb: number): void {
    if (Number.isSafeInteger(mb) && mb >= 1 && mb <= MAX_ANNOUNCED_MB) this.uploadMb = mb
  }

  private uploadBytes(): number | null {
    return this.uploadMb === 0 ? null : this.uploadMb * 2 ** 20
  }

  /**
   * Largest request body for `path` (without the query): an upload follows the
   * announced limit plus multipart framing, avatars and JSON routes have their
   * own small caps. Before any announcement the fixed cap applies to uploads.
   */
  requestCap(path: string): number {
    const kind = routeKind(path)
    if (kind === 'picture') return MAX_AVATAR_BODY
    if (kind === 'json') return MAX_JSON_BODY
    // Never above what the memory budget can lend: a larger body could
    // only wait for permits that do not exist.
    const n = this.uploadBytes()
    return Math.min(n === null ? MAX_BODY : n + MULTIPART_OVERHEAD, LARGE_BODY_BUDGET)
  }

  /**
   * What `forward` enforces for `path`: `requestCap`, but never more than the
   * memory budget can lend. Above it a retry could never succeed, so the body is
   * refused as too large rather than as a busy proxy. `file` tells a file body
   * from a JSON one for the wording of the refusal.
   */
  bodyLimit(path: string): { cap: number, file: boolean } {
    const cap = this.requestCap(path)
    return { cap: Math.min(cap, LARGE_BODY_BUDGET), file: routeKind(path) !== 'json' }
  }

  /**
   * Largest answer the proxy hands to the page: files up to the larger of the
   * old cap and the upload limit (a file stays downloadable after the Owner
   * lowered the limit), bounded by the memory budget.
   */
  responseCap(): number {
    const n = this.uploadBytes()
    const announced = n === null ? 0 : n + MULTIPART_OVERHEAD
    return Math.min(Math.max(MAX_BODY, announced), LARGE_BODY_BUDGET)
  }

  /** Lends memory for a body of `len` bytes. Small bodies take nothing; a large one waits for others to finish. `null` means no memory was available. */
  async lend(len: number): Promise<(() => void) | null> {
    if (len <= SMALL_BODY) return () => {}
    const units = Math.min(Math.ceil(len / BUDGET_UNIT), 0xffffffff)
    return this.budget.acquire(units, this.waitMs)
  }
}
