// Runs in the page of the phone app before the page's own script (an
// initialization script of the Android host; the page is the verified web
// bundle).
//
// Android's WebView hands a request to the app's own scheme to the host
// without its body, so a login or a message would reach the server empty.
// This sends every request with a body to the app's own /api/ as one IPC call
// instead (`mobile_api`); the host forwards it through the same proxy as
// every other request and the answer becomes the page's Response. Requests
// without a body, and every other address, are left to the WebView.
//
// Spike: fetch only. The web app's upload with progress uses XMLHttpRequest
// (shared/lib/xhrUpload.ts), which phase 1 bridges the same way.
(() => {
  'use strict'
  const nativeFetch = window.fetch
  if (typeof nativeFetch !== 'function') return

  const bridged = (request) => {
    if (request.method === 'GET' || request.method === 'HEAD') return false
    try {
      const url = new URL(request.url)
      return url.protocol === window.location.protocol && url.host === window.location.host && url.pathname.startsWith('/api/')
    } catch {
      return false
    }
  }

  const toBase64 = (buffer) => {
    const bytes = new Uint8Array(buffer)
    let text = ''
    for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
    return btoa(text)
  }

  const fromBase64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0))

  window.fetch = async function fetch(input, init) {
    const request = new Request(input, init)
    const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke
    if (!bridged(request) || typeof invoke !== 'function') return nativeFetch.call(this, input, init)
    const url = new URL(request.url)
    const answer = await invoke('mobile_api', {
      method: request.method,
      path: url.pathname + url.search,
      headers: Array.from(request.headers.entries()),
      body: toBase64(await request.arrayBuffer()),
    })
    const empty = answer.status === 204 || answer.status === 304
    return new Response(empty ? null : fromBase64(answer.body), { status: answer.status, headers: answer.headers })
  }
})()
