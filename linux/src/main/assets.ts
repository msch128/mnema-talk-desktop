import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import type { Asset } from './protocol.ts'

const TYPES: Record<string, string> = {
  html: 'text/html; charset=utf-8',
  css: 'text/css; charset=utf-8',
  js: 'text/javascript; charset=utf-8',
}

/** The pages of the host itself (update window, screen picker), read once from the application package. */
export function loadAssets(dir: string): Map<string, Asset> {
  const files = new Map<string, Asset>()
  for (const name of readdirSync(dir)) {
    const type = TYPES[name.slice(name.lastIndexOf('.') + 1)]
    if (type) files.set(name, { type, body: readFileSync(join(dir, name)) })
  }
  return files
}
