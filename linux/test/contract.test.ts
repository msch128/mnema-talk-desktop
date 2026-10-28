import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { APP_CSP, LEGACY_APP_CSP } from '../src/core/proxy.ts'

// The Tauri host and this client implement one host contract twice. These
// tests read both sources, so a limit, policy or list changed on one side only
// fails here, the way `pinned.test.ts` pins the keys.
const rust = (name: string) => readFileSync(new URL(`../../experimental/app/src/${name}`, import.meta.url), 'utf8')
const ts = (name: string) => readFileSync(new URL(`../src/${name}`, import.meta.url), 'utf8')

function rustConst(source: string, name: string): string | undefined {
  return new RegExp(`const ${name}: [A-Za-z0-9]+ = ([^;]+);`).exec(source)?.[1]?.replace(/\s+/g, ' ').trim()
}

function tsConst(source: string, name: string): string | undefined {
  return new RegExp(`const ${name} = ([^\\n]+)`).exec(source)?.[1]?.trim()
}

test('the size and time limits are the ones of the Tauri host', () => {
  const pairs: [string, string, string[]][] = [
    ['web_proxy.rs', 'core/proxy.ts', ['MAX_BODY', 'MAX_JSON_BODY', 'MULTIPART_OVERHEAD', 'MAX_AVATAR_BODY', 'SMALL_BODY', 'LARGE_BODY_BUDGET', 'BUDGET_UNIT', 'MAX_ANNOUNCED_MB']],
    ['web_ws.rs', 'core/sockets.ts', ['MAX_FRAME', 'MAX_SOCKETS', 'MAX_OUTGOING']],
    ['web_updates.rs', 'core/updates.ts', ['MAX_RELEASE_JSON', 'MAX_DOWNLOAD']],
  ]
  for (const [rustFile, tsFile, names] of pairs) {
    const r = rust(rustFile)
    const t = ts(tsFile)
    for (const name of names) {
      assert.ok(rustConst(r, name) !== undefined, `${name} in ${rustFile}`)
      assert.equal(tsConst(t, name), rustConst(r, name), name)
    }
  }
})

/** The string literals of a Rust `concat!(...)` constant, joined. */
function rustConcat(name: string): string {
  const body = new RegExp(`const ${name}: &str = concat!\\(([^)]*)\\)`).exec(rust('web_proxy.rs'))?.[1] ?? ''
  return [...body.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]).join('')
}

test('the content security policies are the ones of the Tauri host, except for its IPC schemes', () => {
  const ipc = "connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost"
  for (const [name, own] of [['APP_CSP', APP_CSP], ['LEGACY_APP_CSP', LEGACY_APP_CSP]] as const) {
    const host = rustConcat(name)
    assert.ok(host.includes(ipc), `${name} keeps the IPC schemes`)
    assert.equal(host.replace(ipc, "connect-src 'self'"), own, name)
  }
})

test('the permissions policy, the forwarded headers and the capabilities are the ones of the Tauri host', () => {
  const policy = /\['permissions-policy', '([^']+)'\]/.exec(ts('core/proxy.ts'))?.[1]
  assert.ok(policy)
  assert.ok(rust('web_proxy.rs').includes(`"permissions-policy".into(), "${policy}".into()`))
  const list = (source: string, pattern: RegExp) => [...(pattern.exec(source)?.[1] ?? '').matchAll(/"([^"]+)"|'([^']+)'/g)].map((m) => m[1] ?? m[2]).sort()
  for (const name of ['FORWARDED_REQUEST', 'FORWARDED_RESPONSE']) {
    const own = list(ts('core/proxy.ts'), new RegExp(`const ${name} = new Set\\(\\[([^\\]]*)\\]\\)`))
    assert.ok(own.length > 0, name)
    assert.deepEqual(own, list(rust('web_proxy.rs'), new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`)), name)
  }
  const caps = list(ts('main/ipc.ts'), /from\.role === 'instance' \? \[([^\]]*)\]/)
  assert.deepEqual(caps, list(rust('web_client.rs'), /if role == "instance" \{\s*&\[([^\]]*)\]/))
  assert.ok(caps.includes('ws_query'))
})
