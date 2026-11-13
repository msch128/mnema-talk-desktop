/**
 * A download of the app's own API, taken over by the host. The page's
 * `<a download>` would go through the proxy, which holds a whole answer in
 * memory (an export has no length and the proxy keeps such an answer to 32 MiB),
 * and nothing would tell the person where the file went or that it failed. Here
 * the answer is requested like the proxy requests it (the session's cookie, the
 * instance's origin, only `/api/` of that server) and streamed to a file next to
 * the target, which is renamed into place only once the whole body has arrived.
 *
 * Everything that can fail on this machine comes first: the target is chosen,
 * its folder made and the temporary file created before the server is asked,
 * because the server spends a data export once it sent the last byte.
 */
import { randomBytes } from 'node:crypto'
import { createWriteStream } from 'node:fs'
import { mkdir, open, readdir, rename, rm, stat } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { downloadName } from './downloadname.ts'
import { SENTENCES, cleanMessage, diskSentence, reasonOf, refusalMessage } from './downloadtext.ts'
import { type HttpResponse, readBody, send } from './http.ts'
import { type Instance, requestHeaders, target } from './proxy.ts'

/** The server's own ceiling for a file (`MAX_UPLOAD_SIZE_MB` is at most 2048), with its overhead. */
export const DOWNLOAD_CAP = (2048 + 1) * 2 ** 20
/** How long the instance may stay silent while the body is being written. */
export const IDLE_MS = 60_000
/** The end of the hidden file a transfer is written to before it gets its name. */
export const PARTIAL_SUFFIX = '.mnema-download'

export interface DownloadLimits { cap: number, idleMs: number }

export type Saved = { ok: true, bytes: number } | { ok: false, error: string }

const fail = (error: string): Saved => ({ ok: false, error })

/** Why a file could not be written, in plain words. */
function diskError(error: unknown): Saved {
  return fail(diskSentence(reasonOf((error as { code?: unknown } | null)?.code)))
}

/** The `message` of the server's error envelope (or a bare `message`), cleaned. */
function serverMessage(body: Uint8Array | null): string | null {
  try {
    const value: unknown = JSON.parse(Buffer.from(body ?? new Uint8Array()).toString('utf8'))
    const inner = typeof value === 'object' && value !== null && 'error' in value ? (value as { error: unknown }).error : value
    const message = typeof inner === 'object' && inner !== null ? (inner as { message?: unknown }).message : undefined
    return typeof message === 'string' ? cleanMessage(message) : null
  } catch {
    return null
  }
}

/**
 * Removes the hidden files of transfers that ended without cleaning up (the
 * client was closed mid-download). A running transfer writes at least once per
 * idle limit, so a file untouched for longer is nobody's.
 */
export async function removeStalePartials(dir: string, olderThanMs: number): Promise<void> {
  let names: string[]
  try {
    names = await readdir(dir)
  } catch {
    return
  }
  for (const name of names) {
    if (!name.startsWith('.') || !name.endsWith(PARTIAL_SUFFIX)) continue
    const path = join(dir, name)
    try {
      if (Date.now() - (await stat(path)).mtimeMs > olderThanMs) await rm(path, { force: true })
    } catch { /* gone already */ }
  }
}

/** Calls that touch the disk; a network error (a read of the socket) is a cut transfer instead. */
const DISK_CALLS = new Set(['open', 'write', 'rename', 'close', 'fsync', 'mkdir'])

/** Streams the body into the file at `temp` (made already), each piece within the idle time, the whole within the cap and the declared length. */
async function write(response: HttpResponse, temp: string, limits: DownloadLimits): Promise<Saved> {
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
    // `flush`: the bytes are on the disk before the file is closed.
    await pipeline(response.stream, count, createWriteStream(temp, { flags: 'r+', flush: true }))
    if (response.contentLength !== null && bytes !== response.contentLength) return fail(SENTENCES.cut)
    return { ok: true, bytes }
  } catch (error) {
    response.destroy()
    const { code, syscall } = (error ?? {}) as { code?: unknown, syscall?: unknown }
    if (code === 'DOWNLOAD_TOO_LARGE') return fail(SENTENCES.too_large)
    return typeof syscall === 'string' && DISK_CALLS.has(syscall) ? diskError(error) : fail(SENTENCES.cut)
  } finally {
    clearTimeout(idle)
  }
}

/** The answer to a refusal is small JSON; anything longer is not read. */
const REFUSAL_BODY = 64 << 10

/** Sends the GET and, for a 200, writes its body into `temp`. */
async function fetchInto(instance: Instance, url: URL, temp: string, limits: DownloadLimits): Promise<Saved> {
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
    return fail(SENTENCES.unreachable)
  }
  // Until the body is piped, a connection that breaks must not raise an unhandled error.
  response.stream.on('error', () => undefined)
  instance.jar.store(response.setCookie, instance.origin.protocol === 'https:', instance.now())
  if (response.status !== 200) {
    const body = await readBody(response, REFUSAL_BODY)
    response.destroy()
    return fail(refusalMessage(response.status, serverMessage(body)))
  }
  if (response.contentLength !== null && response.contentLength > limits.cap) {
    response.destroy()
    return fail(SENTENCES.too_large)
  }
  return write(response, temp, limits)
}

/**
 * Saves `pathAndQuery` of the instance at `dest` (already chosen: by the
 * person in the save dialog, or the test folder). The folder is made and the
 * temporary file created before the request; the file gets its name only when
 * complete, and a failure leaves nothing behind.
 */
export async function downloadInto(instance: Instance, pathAndQuery: string, dest: string, limits: DownloadLimits = { cap: DOWNLOAD_CAP, idleMs: IDLE_MS }): Promise<Saved> {
  const url = target(instance.origin, pathAndQuery)
  if (!url) return fail(SENTENCES.bad_path)
  const dir = dirname(dest)
  const temp = join(dir, `.${randomBytes(16).toString('hex')}${PARTIAL_SUFFIX}`)
  try {
    await mkdir(dir, { recursive: true })
    await removeStalePartials(dir, limits.idleMs * 2)
    await (await open(temp, 'wx')).close()
  } catch (error) {
    return diskError(error)
  }
  let saved = await fetchInto(instance, url, temp, limits)
  if (saved.ok) {
    try {
      // The person chose this path in the save dialog (which asked before replacing a file).
      await rename(temp, dest)
    } catch (error) {
      saved = diskError(error)
    }
  }
  await rm(temp, { force: true }).catch(() => undefined)
  return saved
}

/**
 * A download as the host runs it: the place is chosen first (`choose` gets the
 * proposed name: the page's `download` attribute or the last segment of the
 * address, made plain), then the server is asked. A cancelled choice asks the
 * server for nothing, so a data export is not spent.
 */
export async function saveDownload(instance: Instance, request: DownloadRequest, choose: (proposed: string) => Promise<string | null>, limits?: DownloadLimits): Promise<{ ok: true, bytes: number, dest: string } | { ok: false, error: string } | { cancelled: true }> {
  if (!target(instance.origin, request.path)) return { ok: false, error: SENTENCES.bad_path }
  const dest = await choose(downloadName(null, request.name, request.path)).catch(() => null)
  if (dest === null) return { cancelled: true }
  const saved = await downloadInto(instance, request.path, dest, limits)
  return saved.ok ? { ...saved, dest } : saved
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
