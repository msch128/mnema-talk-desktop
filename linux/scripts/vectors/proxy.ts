import { verifyBundle } from '../../src/core/bundle.ts'
import { Jar } from '../../src/core/cookies.ts'
import { Limits, clampRange, inlineSafe, mimeFor, serveBundle, target } from '../../src/core/proxy.ts'
import { bundle, spec, trusted } from '../../test/helpers/fixtures.ts'
import { b64, keysOf, type Json } from './common.ts'

export function proxyVectors(): Json {
  const types = ['application/json', 'text/plain; charset=utf-8', 'image/png', 'IMAGE/JPEG', 'image/webp', 'image/avif', 'image/x-icon', 'audio/ogg', 'video/mp4', 'application/vnd.api+json', 'application/octet-stream',
    'text/html', 'image/svg+xml', 'application/xml', 'text/javascript', 'application/javascript', 'application/pdf', 'text/xml', 'application/xhtml+xml', 'text/css', 'application/json+x', '', 'multipart/x-mixed-replace']
    .map((input) => ({ input, safe: inlineSafe(input) }))
  const origin = 'https://community.example'
  const targets = ['/api/channels?x=1', '/api/', '/index.html', '/api', '/api/../x', '/api/%2e%2e/x', '/api/a\\b', '/api/a\nb', '//evil.example/api/x', '/apix', 'https://evil.example/api/x', '/api/\u0085', '/api/a b', '/api/%2Fx', '/api/./x']
    .map((input) => ({ origin, input, href: target(new URL(origin), input)?.href ?? null }))
  const upload = '/api/channels/00000000-0000-4000-8000-000000000000/upload'
  const capSteps = [
    { announce: [], path: upload }, { announce: [10], path: upload }, { announce: [10], path: '/api/channels/x/messages' }, { announce: [10], path: '/api/users/me/avatar' }, { announce: [10], path: '/api/wiki/images' }, { announce: [10], path: '/api/wiki/images/x' },
    { announce: [10], path: '/api/channels//upload' }, { announce: [10], path: '/api/channels/a/b/upload' }, { announce: [512], path: upload }, { announce: [512, 0, 999_999], path: upload }, { announce: [2048], path: upload },
    { announce: [1], path: upload }, { announce: [2049], path: upload },
  ].map((c) => {
    const limits = new Limits()
    for (const mb of c.announce) limits.announceUploadMb(mb)
    return { ...c, request_cap: limits.requestCap(c.path), response_cap: limits.responseCap() }
  })
  const ranges = ['bytes=0-', 'bytes=100-', 'bytes=0-99', 'bytes=0-1073741823', 'bytes=-500', 'bytes=-1073741824', 'bytes=18446744073709551615-', 'bytes=0-1,5-', 'items=0-', 'bytes=a-b', 'bytes=9-3', 'bytes=-', '']
    .map((input) => ({ input, clamped: clampRange(input) }))
  const mimes = ['index.html', 'a.JS', 'a.mjs', 'a.css', 'a.json', 'a.map', 'a.wasm', 'a.svg', 'a.png', 'a.jpg', 'a.jpeg', 'a.webp', 'a.gif', 'a.ico', 'a.woff2', 'a.woff', 'a.txt', 'a.webmanifest', 'a.mp3', 'a.ogg', 'a.oga', 'a.wav', 'a.bin', 'noext', 'a.tar.gz']
    .map((name) => ({ name, type: mimeFor(name) }))

  const sb = spec('0.8.0', { files: [['index.html', '<html>app</html>'], ['assets/app-1.js', 'console.log(1)'], ['logo.svg', '<svg/>'], ['sw.js', 'x']] })
  const zip = bundle(sb)
  const served = verifyBundle(zip, trusted(7))
  const serveCases = [['GET', '/'], ['GET', '/index.html'], ['GET', '/assets/app-1.js'], ['GET', '/assets/missing.js'], ['GET', '/c/11111111-1111-1111-1111-111111111111'], ['GET', '//index.html'], ['GET', '/logo.svg'],
    ['GET', '/sw.js'], ['HEAD', '/'], ['HEAD', '/assets/app-1.js'], ['POST', '/'], ['DELETE', '/assets/app-1.js'], ['GET', '/nothing/here']]
    .map(([method, path]) => {
      const r = serveBundle(served, method as string, path as string)
      const h = (n: string) => r.headers.find(([k]) => k === n)?.[1] ?? null
      return { method, path, status: r.status, content_type: h('content-type'), cache_control: h('cache-control'), allow: h('allow'), body: Buffer.from(r.body).toString() }
    })

  const jarSteps: { name: string, steps: Json[] }[] = [
    {
      name: 'prefixes, the secure flag, sizes and names',
      steps: [
        { op: 'store', https: false, now: 0, set_cookie: ['__Host-a=1; Max-Age=60', '__Host-b=1; Secure; Path=/x; Max-Age=60', '__Host-c=1; Secure; Path=/; Domain=example.org; Max-Age=60', '__Secure-d=1; Max-Age=60', 'e=1; Secure; Max-Age=60', '=nameless; Max-Age=60', 'not a cookie'] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: ['__Host-ok=1; Secure; Path=/; Max-Age=60'] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: [`big=${'x'.repeat(5000)}; Max-Age=60`] },
        { op: 'header', now: 1 },
        { op: 'store', https: true, now: 0, set_cookie: ['__Secure-s=2; Secure; Max-Age=60', 'plain=3; Max-Age=60; HttpOnly; SameSite=Lax'] },
        { op: 'header', now: 1 },
      ],
    },
    {
      name: 'expiry, replacement and logout',
      steps: [
        { op: 'store', https: true, now: 100, set_cookie: ['keep=1; Max-Age=1000', 'gone=1; Max-Age=10', 'session=1', 'late=1; Max-Age=500'] },
        { op: 'header', now: 101 },
        { op: 'header', now: 111 },
        { op: 'header', now: 1101 },
        { op: 'store', https: true, now: 200, set_cookie: ['keep=2; Max-Age=1000'] },
        { op: 'header', now: 201 },
        { op: 'store', https: true, now: 300, set_cookie: ['keep=; Max-Age=0', 'session=; Max-Age=0'] },
        { op: 'header', now: 301 },
      ],
    },
  ]
  const jars = jarSteps.map((j) => {
    const jar = new Jar(null, null, 0)
    return {
      name: j.name,
      steps: j.steps.map((s) => {
        if (s.op === 'store') { jar.store(s.set_cookie as string[], s.https as boolean, s.now as number); return s }
        return { ...s, expect: jar.header(s.now as number) }
      }),
    }
  })

  return { inline_safe: types, targets, caps: capSteps, ranges, mimes, serve: { trusted: keysOf(7), zip: b64(zip), cases: serveCases }, jars }
}
