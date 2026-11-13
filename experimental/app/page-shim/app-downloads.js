// Runs in the page of a server before the page's own script (an
// initialization script of the host; the page is the verified web bundle).
//
// A download link of the app's own API (`<a href="/api/..." download>`: the
// data export, an attachment from the media viewer) is saved by the host, not
// by the webview. The webview would fetch it through the host's in-memory
// proxy and then save it on its own terms: WKWebView over the app's custom
// scheme, WebView2 behind the host's request handler, each without a word
// when that fails, and the server counts a download as done once it sent the
// last byte. The host streams the file to disk, says where it went and shows
// an error when it could not be saved. Everything else (other links, `blob:`
// files the page made itself, clicks with a modifier) stays the webview's.
(() => {
  'use strict'
  if (window !== window.top) return
  const invoke = (path, name) => {
    const internals = window.__TAURI_INTERNALS__
    if (!internals || typeof internals.invoke !== 'function') return false
    // The host reports the outcome itself; the page has nothing to add.
    internals.invoke('desktop_download', { path, name }).catch(() => {})
    return true
  }

  const ownApi = (href) => {
    try {
      const url = new URL(href, window.location.href)
      const own = url.protocol === window.location.protocol && url.host === window.location.host
      return own && url.pathname.startsWith('/api/') ? url.pathname + url.search : null
    } catch {
      return null
    }
  }

  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    const target = event.target
    const link = target && typeof target.closest === 'function' ? target.closest('a[download]') : null
    if (!link) return
    const path = ownApi(link.getAttribute('href') ?? '')
    if (path === null) return
    if (invoke(path, link.getAttribute('download') ?? '')) event.preventDefault()
  }, true)
})()
