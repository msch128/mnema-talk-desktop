// The page shim that turns a picked file into bytes before the request leaves
// the page (experimental/app/page-shim/buffered-bodies.js), run against a fake
// `fetch` that records what the host would be handed.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const script = readFileSync(resolve(process.cwd(), '../page-shim/buffered-bodies.js'), 'utf8')

let native: ReturnType<typeof vi.fn>

function install() {
  native = vi.fn(() => Promise.resolve(new Response('{}')))
  window.fetch = native as unknown as typeof window.fetch
  new Function(script)()
}

function sent(call = 0) {
  const [input, init] = native.mock.calls[call] as [string, RequestInit]
  return { input, init, headers: new Headers(init.headers), body: init.body as Uint8Array }
}

/** A picture-like byte string that no text handling would leave alone. */
function image(size: number) {
  return Uint8Array.from({ length: size }, (_, i) => (i * 31 + (i >> 8)) & 0xff)
}

function indexOf(haystack: Uint8Array, needle: Uint8Array, from = 0) {
  outer: for (let at = from; at <= haystack.length - needle.length; at++) {
    for (let i = 0; i < needle.length; i++) if (haystack[at + i] !== needle[i]) continue outer
    return at
  }
  return -1
}

/** The bytes between the blank line after the headers of the first part and the next boundary. */
function firstPart(body: Uint8Array, boundary: string) {
  const text = new TextEncoder()
  const start = indexOf(body, text.encode('\r\n\r\n')) + 4
  const end = indexOf(body, text.encode(`\r\n--${boundary}`), start)
  return { head: new TextDecoder().decode(body.subarray(0, start)), data: body.subarray(start, end) }
}

describe('files in a request body', () => {
  beforeEach(() => {
    ;(window as unknown as { happyDOM: { setURL(url: string): void } }).happyDOM.setURL('https://localhost/')
    install()
  })

  for (const size of [1 << 10, (1 << 20) - 1, (1 << 20) + 1, (3 << 20) + 17]) {
    it(`sends a ${size} byte picture of a form as the bytes of one multipart body`, async () => {
      const bytes = image(size)
      const form = new FormData()
      form.append('avatar', new File([bytes], 'me "1".png', { type: 'image/png' }))
      form.append('note', 'a\nb')
      await window.fetch('/api/users/me/avatar', { method: 'POST', credentials: 'same-origin', body: form })

      const { init, headers, body } = sent()
      expect(init.method).toBe('POST')
      expect(init.credentials).toBe('same-origin')
      expect(body).toBeInstanceOf(Uint8Array)
      const boundary = /^multipart\/form-data; boundary=(.+)$/.exec(headers.get('content-type') ?? '')?.[1]
      expect(boundary).toBeTruthy()
      const { head, data } = firstPart(body, boundary as string)
      expect(head).toContain('Content-Disposition: form-data; name="avatar"; filename="me %221%22.png"')
      expect(head).toContain('Content-Type: image/png')
      expect(Buffer.from(data).equals(Buffer.from(bytes))).toBe(true)
      const tail = new TextDecoder().decode(body.subarray(body.length - 200))
      expect(tail).toContain('name="note"\r\n\r\na\r\nb\r\n')
      expect(tail.endsWith(`--${boundary}--\r\n`)).toBe(true)
    })
  }

  it('sends a blob as its bytes under its own type and keeps a header the page set', async () => {
    const bytes = image(4096)
    await window.fetch('/api/channels/1/upload', { method: 'PUT', body: new Blob([bytes], { type: 'image/webp' }) })
    expect(sent().headers.get('content-type')).toBe('image/webp')
    expect(Buffer.from(sent().body).equals(Buffer.from(bytes))).toBe(true)

    await window.fetch('/api/channels/1/upload', { method: 'PUT', headers: { 'content-type': 'application/x-custom', 'x-a': '1' }, body: new Blob([bytes]) })
    expect(sent(1).headers.get('content-type')).toBe('application/x-custom')
    expect(sent(1).headers.get('x-a')).toBe('1')
  })

  it('leaves everything that already is bytes, and every other origin, to the browser', async () => {
    const form = new FormData()
    form.append('a', 'b')
    const json = { method: 'POST', body: '{"a":1}' }
    await window.fetch('/api/messages', json)
    expect(native).toHaveBeenLastCalledWith('/api/messages', json)
    const buffer = { method: 'POST', body: new Uint8Array([1, 2]) }
    await window.fetch('/api/x', buffer)
    expect(native).toHaveBeenLastCalledWith('/api/x', buffer)
    const foreign = { method: 'POST', body: form }
    await window.fetch('https://other.example/api/x', foreign)
    expect(native).toHaveBeenLastCalledWith('https://other.example/api/x', foreign)
    await window.fetch('/api/health')
    expect(native).toHaveBeenLastCalledWith('/api/health', undefined)
  })

  it('passes the failure of an unreadable file on instead of sending an empty body', async () => {
    const gone = new File(['x'], 'gone.png')
    vi.spyOn(Blob.prototype, 'arrayBuffer').mockRejectedValue(new DOMException('gone', 'NotFoundError'))
    const form = new FormData()
    form.append('avatar', gone)
    await expect(window.fetch('/api/users/me/avatar', { method: 'POST', body: form })).rejects.toThrow('gone')
    expect(native).not.toHaveBeenCalled()
  })
})
