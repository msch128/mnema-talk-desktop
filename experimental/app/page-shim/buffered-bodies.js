// Runs in every page of the app before the page's own script (an
// initialization script of the host; the page is the verified web bundle).
//
// A page sends a picked file as `FormData` or `Blob`: the body of that request
// is a reference to the file, which the browser resolves while it sends. The
// webviews of the desktop clients hand a request for the app's own scheme to
// the host before that happens, and WebView2 (Windows) passes on an empty
// body for it, so the server saw no file ("choose a file first") although the
// same page works in a browser. This turns such a body into the bytes it
// stands for, read by the page itself, before the request leaves it. The
// request is otherwise the page's own: same URL, method and headers (the
// content type of a form gets its boundary), and only requests to the page's
// own origin are touched. Strings, buffers and streams already are bytes.
(() => {
  'use strict'
  const nativeFetch = window.fetch
  if (typeof nativeFetch !== 'function' || typeof FormData !== 'function' || typeof Blob !== 'function') return
  const text = new TextEncoder()

  const own = (input) => {
    try {
      const url = new URL(typeof input === 'string' || input instanceof URL ? String(input) : input.url, window.location.href)
      return url.protocol === window.location.protocol && url.host === window.location.host
    } catch {
      return false
    }
  }

  // A name or file name inside the quotes of a part header (HTML, "multipart/form-data").
  const quoted = (value) => String(value).replace(/\r/g, '%0D').replace(/\n/g, '%0A').replace(/"/g, '%22')
  const crlf = (value) => String(value).replace(/\r\n|\r|\n/g, '\r\n')

  const boundary = () => {
    const random = new Uint8Array(16)
    crypto.getRandomValues(random)
    return `----MnemaForm${Array.from(random, (b) => b.toString(16).padStart(2, '0')).join('')}`
  }

  async function bytesOfForm(form) {
    const edge = boundary()
    const parts = []
    let length = 0
    const push = (chunk) => {
      parts.push(chunk)
      length += chunk.length
    }
    for (const [name, value] of form.entries()) {
      if (typeof value === 'string') {
        push(text.encode(`--${edge}\r\nContent-Disposition: form-data; name="${quoted(name)}"\r\n\r\n${crlf(value)}\r\n`))
      } else {
        const type = value.type || 'application/octet-stream'
        push(text.encode(`--${edge}\r\nContent-Disposition: form-data; name="${quoted(name)}"; filename="${quoted(value.name)}"\r\nContent-Type: ${type}\r\n\r\n`))
        push(new Uint8Array(await value.arrayBuffer()))
        push(text.encode('\r\n'))
      }
    }
    push(text.encode(`--${edge}--\r\n`))
    const bytes = new Uint8Array(length)
    let at = 0
    for (const chunk of parts) {
      bytes.set(chunk, at)
      at += chunk.length
    }
    return { bytes, type: `multipart/form-data; boundary=${edge}` }
  }

  async function bytesOf(body) {
    if (body instanceof FormData) return bytesOfForm(body)
    return { bytes: new Uint8Array(await body.arrayBuffer()), type: body.type }
  }

  window.fetch = function fetch(input, init) {
    const body = init ? init.body : undefined
    if (!(body instanceof FormData || body instanceof Blob) || !own(input)) {
      return nativeFetch.call(window, input, init)
    }
    return bytesOf(body).then(({ bytes, type }) => {
      const headers = new Headers(init.headers)
      if (body instanceof FormData || (type && !headers.has('content-type'))) headers.set('content-type', type)
      return nativeFetch.call(window, input, { ...init, headers, body: bytes })
    })
  }
})()
