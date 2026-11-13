/**
 * A download of the app's own API, taken over by the host. The page's
 * `<a download>` would go through the proxy, which holds a whole answer in
 * memory (an export has no length and the proxy keeps such an answer to 32 MiB),
 * and nothing would tell the person where the file went or that it failed. Here
 * the answer is requested like the proxy requests it (the session's cookie, the
 * instance's origin, only `/api/` of that server) and streamed to a file next to
 * the target, which is renamed into place only once the whole body has arrived.
 * Nothing is read into memory, and a failed transfer leaves no file behind.
 */
import { randomBytes } from 'node:crypto'
import { createWriteStream } from 'node:fs'
import { rename, rm } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { downloadName } from './downloadname.ts'
import { type HttpResponse, headerText, readBody, send } from './http.ts'
import { type Instance, requestHeaders, target } from './proxy.ts'

/** The server's own ceiling for a file (`MAX_UPLOAD_SIZE_MB` is at most 2048), with its overhead. */
export const DOWNLOAD_CAP = (2048 + 1) * 2 ** 20
/** How long the instance may stay silent while the body is being written. */
export const IDLE_MS = 60_000

export const NOTHING_TO_DOWNLOAD = 'There is nothing to download: the file was downloaded already or has expired.'
export const INTERRUPTED = 'The download was interrupted; nothing was saved.'
export const TOO_LARGE = 'The file is larger than this client accepts; nothing was saved.'
const UNREACHABLE = 'The server could not be reached.'
const BAD_ADDRESS = 'This address cannot be downloaded.'

export interface DownloadLimits { cap: number, idleMs: number }

/** What a person is told of a refusal: the 404 of a spent file, the server's own message, or the status. */
export function refusalMessage(status: number, body: Uint8Array | null): string {
  if (status === 404) return NOTHING_TO_DOWNLOAD
  try {
    const value: unknown = JSON.parse(Buffer.from(body ?? new Uint8Array()).toString('utf8'))
    const inner = typeof value === 'object' && value !== null && 'error' in value ? (value as { error: unknown }).error : value
    const message = typeof inner === 'object' && inner !== null ? (inner as { message?: unknown }).message : undefined
    if (typeof message === 'string') {
      const text = [...message].filter((c) => (c.codePointAt(0) as number) >= 0x20).join('').trim().slice(0, 300)
      if (text !== '') return text
    }
  } catch { /* not JSON */ }
  return `The download failed (HTTP ${status}).`
}

/** Why a file could not be written, without any path. */
const DISK_REASONS: Record<string, string> = {
  ENOSPC: 'there is no space left on the device',
  EACCES: 'permission denied',
  EPERM: 'permission denied',
  EROFS: 'the folder is read-only',
  ENAMETOOLONG: 'the name is too long',
  EISDIR: 'the target is a folder',
  ENOENT: 'the folder does not exist',
  EDQUOT: 'the disk quota is exceeded',
}

const DISK_CALLS = new Set(['open', 'write', 'rename', 'close', 'fsync', 'mkdir'])

function explain(error: unknown): string {
  const { code, syscall } = (error ?? {}) as { code?: unknown, syscall?: unknown }
  if (code === 'DOWNLOAD_TOO_LARGE') return TOO_LARGE
  if (typeof syscall === 'string' && DISK_CALLS.has(syscall)) {
    return `The file could not be saved: ${typeof code === 'string' ? (DISK_REASONS[code] ?? code) : 'an unknown error'}.`
  }
  return INTERRUPTED
}

export type Saved = { ok: true, bytes: number } | { ok: false, error: string }

/** An answer that is on its way: the name it gets, and one of two ways to end it. */
export interface Pending {
  name: string
  /** Writes the body to `dest` (through a temporary file next to it). */
  save(dest: string): Promise<Saved>
  /** Drops the transfer; nothing is written. */
  abandon(): void
}

export type Opened = { pending: Pending } | { error: string }

/** Streams `response` into `dest`. The partial file never outlives a failure. */
async function save(response: HttpResponse, dest: string, limits: DownloadLimits): Promise<Saved> {
  const temp = join(dirname(dest), `.mnema-${randomBytes(6).toString('hex')}.part`)
  let bytes = 0
  let idle: NodeJS.Timeout | undefined
  const arm = () => {
    clearTimeout(idle)
    idle = setTimeout(() => response.stream.destroy(new Error('idle')), limits.idleMs)
  }
  const count = new Transform({
    transform(chunk: Buffer, _encoding, done) {
      arm()
      bytes += chunk.length
      if (bytes > limits.cap) done(Object.assign(new Error('too large'), { code: 'DOWNLOAD_TOO_LARGE' }))
      else done(null, chunk)
    },
  })
  try {
    arm()
    await pipeline(response.stream, count, createWriteStream(temp, { flags: 'wx' }))
    if (response.contentLength !== null && bytes !== response.contentLength) throw new Error('short body')
    await rename(temp, dest)
    return { ok: true, bytes }
  } catch (error) {
    response.destroy()
    await rm(temp, { force: true }).catch(() => undefined)
    return { ok: false, error: explain(error) }
  } finally {
    clearTimeout(idle)
  }
}

/** The answer to a refusal is small JSON; anything longer is not read. */
const REFUSAL_BODY = 64 << 10

/**
 * GETs `pathAndQuery` of the instance. A refusal is an error sentence, an answer
 * a `Pending` whose body has not been read: the caller asks where to put it first.
 * `attribute` is the page's `download` attribute, a name to use if the server gives none.
 */
export async function openDownload(instance: Instance, pathAndQuery: string, attribute: string, limits: DownloadLimits = { cap: DOWNLOAD_CAP, idleMs: IDLE_MS }): Promise<Opened> {
  const url = target(instance.origin, pathAndQuery)
  if (!url) return { error: BAD_ADDRESS }
  let response: HttpResponse
  try {
    response = await send({
      method: 'GET',
      url,
      headers: requestHeaders(instance, []),
      connectTimeoutMs: 10_000,
      // The idle timeout bounds each piece; the whole transfer only needs an upper bound that is never the reason it ends.
      totalTimeoutMs: 2 ** 31 - 1,
      agent: instance.agent,
    })
  } catch {
    return { error: UNREACHABLE }
  }
  // Until the body is piped, a connection that breaks must not raise an unhandled error.
  response.stream.on('error', () => undefined)
  instance.jar.store(response.setCookie, instance.origin.protocol === 'https:', instance.now())
  if (response.status !== 200) {
    const body = await readBody(response, REFUSAL_BODY)
    response.destroy()
    return { error: refusalMessage(response.status, body) }
  }
  if (response.contentLength !== null && response.contentLength > limits.cap) {
    response.destroy()
    return { error: TOO_LARGE }
  }
  const name = downloadName(headerText(response.headers['content-disposition']) ?? null, attribute, pathAndQuery)
  return { pending: { name, save: (dest) => save(response, dest, limits), abandon: () => response.destroy() } }
}

/** What the page may ask for: a path and query of the server's API and a suggested name, each of sensible length. */
export interface DownloadRequest { path: string, name: string }

const MAX_PATH = 8192
const MAX_NAME = 1024

/**
 * Checks who asks and what for. Only the instance page of a server may; the
 * path is judged again by `target` when the request is made.
 */
export function downloadRequest(role: string | undefined, server: string | null, args: Record<string, unknown>): DownloadRequest {
  if (role !== 'instance' || server === null) throw new Error('Not allowed.')
  const { path, name } = args
  if (typeof path !== 'string' || typeof name !== 'string' || path.length > MAX_PATH || name.length > MAX_NAME || !path.startsWith('/api/')) throw new Error('invalid arguments')
  return { path, name }
}
