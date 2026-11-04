#!/usr/bin/env node
// File size limits, the same numbers as the server (AGENTS.md): production files
// up to 400 lines, test files up to 800. Rust keeps its unit tests in the file they
// test, so a `#[cfg(test)]` module does not count toward the production file; it is
// a test file of its own and may take 800 lines. Function length and cognitive
// complexity are checked where the language tooling can do it: clippy for Rust
// (clippy.toml) and ESLint for TypeScript (linux/eslint.config.ts).
//
// scripts/limits-allowlist.txt lists the files that are over today. It only
// shrinks: an entry for a file that is within its limit now fails the check, so it
// gets removed. Never add an entry; split the file by responsibility instead.
// Usage: node scripts/check-limits.mjs
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const PRODUCTION_LIMIT = 400
export const TEST_LIMIT = 800
const SOURCE = /\.(rs|ts|tsx|js|mjs|vue)$/
const GENERATED = /(^|\/)(node_modules|target|dist|out|bundle|release)\//
const TEST_PATH = /(^|\/)(tests?|page-tests|fixtures)\/|\.(test|spec|fixture)\.[cm]?[jt]sx?$|(^|\/)tests\.rs$/

export const isTestFile = (path) => TEST_PATH.test(path)

const lineCount = (text) => (text === '' ? 0 : text.split('\n').length - (text.endsWith('\n') ? 1 : 0))

/**
 * The size of one file as [production lines, test lines]. In a Rust file the
 * `#[cfg(test)]` modules are counted apart from the rest; the longest one is
 * what is returned as the test size.
 */
export function measure(path, text) {
  if (isTestFile(path)) return { production: 0, test: lineCount(text) }
  if (!path.endsWith('.rs')) return { production: lineCount(text), test: 0 }
  const lines = text.split('\n')
  let production = text === '' ? 0 : lines.length - (text.endsWith('\n') ? 1 : 0)
  let test = 0
  for (let i = 0; i < lines.length; i++) {
    if (lines[i] !== '#[cfg(test)]' || !/^(pub(\([a-z]+\))? )?mod \w+ \{$/.test(lines[i + 1] ?? '')) continue
    let end = i + 2
    while (end < lines.length && lines[end] !== '}') end++
    const size = end - i + 1
    production -= size
    test = Math.max(test, size)
    i = end
  }
  return { production, test }
}

export function parseAllowlist(text) {
  const entries = new Set()
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    if (line && !line.startsWith('#')) entries.add(line)
  }
  return entries
}

/** Files over their limit, as `path production` or `path test` (the kind that is over). */
export function offenders(files) {
  const found = []
  for (const [path, text] of files) {
    const size = measure(path, text)
    if (size.production > PRODUCTION_LIMIT) found.push(`${path} production`)
    if (size.test > TEST_LIMIT) found.push(`${path} test`)
  }
  return found.sort()
}

function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
  const tracked = execFileSync('git', ['ls-files', '-z'], { cwd: root, maxBuffer: 64 * 1024 * 1024 })
    .toString('utf8').split('\0').filter((file) => SOURCE.test(file) && !GENERATED.test(file))
  const found = offenders(tracked.map((file) => [file, readFileSync(join(root, file), 'utf8')]))
  const allowed = parseAllowlist(readFileSync(join(root, 'scripts/limits-allowlist.txt'), 'utf8'))
  const added = found.filter((entry) => !allowed.has(entry))
  const stale = [...allowed].filter((entry) => !found.includes(entry)).sort()
  if (added.length > 0) console.error(`Over the file size limit (${PRODUCTION_LIMIT} production, ${TEST_LIMIT} test lines; split by responsibility):\n  ${added.join('\n  ')}`)
  if (stale.length > 0) console.error(`Within the limit now, remove from scripts/limits-allowlist.txt:\n  ${stale.join('\n  ')}`)
  if (added.length > 0 || stale.length > 0) process.exitCode = 1
  else console.log(`File sizes: ${tracked.length} source files checked, ${allowed.size} over the limit and listed`)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
